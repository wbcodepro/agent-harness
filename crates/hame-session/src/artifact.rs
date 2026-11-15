//! Content-addressed artifact storage.
//!
//! Images, downloads and other binary evidence never go into the session log
//! itself. Their bytes are written here first, atomically and under their
//! SHA-256, and the log records only an [`ArtifactRef`]. A crash between the
//! write and the event that references it leaves an unreferenced file, which
//! garbage collection removes; it can never leave an event pointing at a
//! half-written file.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The largest artifact accepted, in bytes.
pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

/// Media types an artifact may declare. Anything else is stored as
/// `application/octet-stream`, never as a type a provider would render.
pub const KNOWN_MIME: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "application/pdf",
    "text/plain",
    "text/csv",
    "text/html",
    "application/json",
    "application/octet-stream",
];

/// Where pixels in an image came from and how to map a point in the image
/// back to the screen, so a coordinate a model picks in a resized crop lands
/// on the right physical pixel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    /// What was captured, e.g. `desktop:TextEdit:window=1234` or
    /// `browser:tab=3`.
    pub target: String,
    /// Wall-clock milliseconds when the capture was taken.
    pub ts: u64,
    /// The coordinate space `crop` is expressed in: `screen-points`
    /// (macOS logical points), `screen-pixels`, or `css-pixels`.
    pub space: String,
    /// The captured region in `space`: left, top, width, height.
    pub crop: [f64; 4],
    /// Physical pixels per unit of `space` (2.0 on a Retina display).
    #[serde(default = "one")]
    pub scale: f64,
    /// Whether any region was blanked out before storage.
    #[serde(default)]
    pub redacted: bool,
    /// The original artifact this one was resized or redacted from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
}

fn one() -> f64 {
    1.0
}

impl Capture {
    /// Maps a point in an image of `sent` size (width, height in pixels)
    /// to `space`: `x_space = crop_left + x * crop_width / sent_width`.
    pub fn to_space(&self, sent: (u32, u32), point: (f64, f64)) -> (f64, f64) {
        let [left, top, width, height] = self.crop;
        (
            left + point.0 * width / f64::from(sent.0.max(1)),
            top + point.1 * height / f64::from(sent.1.max(1)),
        )
    }
}

/// A stored artifact as events refer to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Lowercase hex SHA-256 of the bytes; also the storage key.
    pub sha256: String,
    pub mime: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A human-readable name, such as a downloaded file's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<Capture>,
}

impl ArtifactRef {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    pub fn short(&self) -> &str {
        &self.sha256[..self.sha256.len().min(12)]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("artifact {0} is missing from the store")]
    Missing(String),
    #[error("artifact {sha256} is corrupt: its bytes hash to {actual}")]
    Corrupt { sha256: String, actual: String },
    #[error("artifact of {0} bytes is larger than the {MAX_ARTIFACT_BYTES}-byte limit")]
    TooLarge(u64),
    #[error("invalid artifact id {0:?}")]
    InvalidId(String),
}

/// Reads artifact bytes. Provider adapters get one of these so they can
/// inline an image without knowing where the store lives.
pub trait ArtifactReader: Send + Sync {
    fn read(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactError>;
}

/// An [`ArtifactReader`] that can be cloned into a request and compared by
/// identity, so request types keep their derives.
#[derive(Clone)]
pub struct ArtifactSource(pub Arc<dyn ArtifactReader>);

impl std::fmt::Debug for ArtifactSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArtifactSource")
    }
}

impl PartialEq for ArtifactSource {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl ArtifactSource {
    pub fn read(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactError> {
        self.0.read(artifact)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Reads the pixel size from a PNG, JPEG, GIF or WebP header without
/// decoding the image.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(&bytes[16..20]), be32(&bytes[20..24])));
    }
    if bytes.len() >= 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([bytes[6], bytes[7]]);
        let h = u16::from_le_bytes([bytes[8], bytes[9]]);
        return Some((w.into(), h.into()));
    }
    if bytes.len() >= 30 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        match &bytes[12..16] {
            b"VP8X" => {
                let w = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
                let h = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
                return Some((w, h));
            }
            b"VP8 " => {
                let w = u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3fff;
                let h = u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3fff;
                return Some((w.into(), h.into()));
            }
            b"VP8L" => {
                let b = &bytes[21..25];
                let w = 1 + ((u32::from(b[1]) & 0x3f) << 8 | u32::from(b[0]));
                let h = 1
                    + ((u32::from(b[3]) & 0xf) << 10 | u32::from(b[2]) << 2 | u32::from(b[1]) >> 6);
                return Some((w, h));
            }
            _ => return None,
        }
    }
    if bytes.len() >= 4 && bytes.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xff {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]);
                return Some((w.into(), h.into()));
            }
            i += 2 + len;
        }
    }
    None
}

/// Guesses a media type from magic bytes; used to reject a declared type
/// that does not match the content.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else {
        None
    }
}

/// A directory of artifacts keyed by hash: `<root>/<aa>/<sha256>`.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_of(&self, sha256: &str) -> Result<PathBuf, ArtifactError> {
        if !valid_id(sha256) {
            return Err(ArtifactError::InvalidId(sha256.to_string()));
        }
        Ok(self.root.join(&sha256[..2]).join(sha256))
    }

    /// Stores `bytes` and returns a reference to them. The bytes are on
    /// stable storage before this returns, so an event appended afterwards
    /// never refers to something a crash could lose.
    ///
    /// A declared image type that the bytes contradict is corrected to what
    /// the bytes are; an unknown type becomes `application/octet-stream`.
    pub fn put(&self, bytes: &[u8], mime: &str) -> Result<ArtifactRef, ArtifactError> {
        let size = bytes.len() as u64;
        if size > MAX_ARTIFACT_BYTES {
            return Err(ArtifactError::TooLarge(size));
        }
        let sniffed = sniff_mime(bytes);
        let mime = match sniffed {
            Some(actual) if mime.starts_with("image/") || mime == "application/pdf" => actual,
            _ if mime.starts_with("image/") => "application/octet-stream",
            _ if KNOWN_MIME.contains(&mime) => mime,
            _ => "application/octet-stream",
        };
        let sha256 = sha256_hex(bytes);
        let path = self.path_of(&sha256)?;
        if !path.exists() {
            let dir = path.parent().expect("artifact paths have a parent");
            fs::create_dir_all(dir)?;
            let tmp = dir.join(format!(".{sha256}.{}.tmp", uuid::Uuid::new_v4().simple()));
            {
                let mut file = fs::File::create(&tmp)?;
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            if let Err(e) = fs::rename(&tmp, &path) {
                let _ = fs::remove_file(&tmp);
                if !path.exists() {
                    return Err(e.into());
                }
            }
            if let Ok(dir) = fs::File::open(dir) {
                let _ = dir.sync_all();
            }
        }
        let (width, height) = match image_dimensions(bytes) {
            Some((w, h)) if mime.starts_with("image/") => (Some(w), Some(h)),
            _ => (None, None),
        };
        Ok(ArtifactRef {
            sha256,
            mime: mime.to_string(),
            bytes: size,
            width,
            height,
            name: None,
            capture: None,
        })
    }

    /// Reads and verifies an artifact's bytes.
    pub fn get(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactError> {
        let path = self.path_of(&artifact.sha256)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ArtifactError::Missing(artifact.sha256.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        let actual = sha256_hex(&bytes);
        if actual != artifact.sha256 {
            return Err(ArtifactError::Corrupt {
                sha256: artifact.sha256.clone(),
                actual,
            });
        }
        Ok(bytes)
    }

    pub fn contains(&self, sha256: &str) -> bool {
        self.path_of(sha256).is_ok_and(|path| path.is_file())
    }
}

impl ArtifactReader for ArtifactStore {
    fn read(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactError> {
        self.get(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn put_is_atomic_content_addressed_and_verified() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(dir.path());
        let a = store.put(PNG_1X1, "image/png").unwrap();
        assert_eq!(a.mime, "image/png");
        assert_eq!((a.width, a.height), (Some(3), Some(2)));
        assert_eq!(store.get(&a).unwrap(), PNG_1X1);
        // Same bytes, same key, no second file.
        assert_eq!(store.put(PNG_1X1, "image/png").unwrap().sha256, a.sha256);
        // A lie about the type is corrected from the bytes.
        assert_eq!(store.put(PNG_1X1, "image/jpeg").unwrap().mime, "image/png");
        assert_eq!(
            store.put(b"not an image", "image/png").unwrap().mime,
            "application/octet-stream"
        );
        // Tampering is detected.
        fs::write(store.path_of(&a.sha256).unwrap(), b"tampered").unwrap();
        assert!(matches!(store.get(&a), Err(ArtifactError::Corrupt { .. })));
        fs::remove_file(store.path_of(&a.sha256).unwrap()).unwrap();
        assert!(matches!(store.get(&a), Err(ArtifactError::Missing(_))));
        assert!(matches!(
            store.path_of("../../etc/passwd"),
            Err(ArtifactError::InvalidId(_))
        ));
    }

    #[test]
    fn crop_transform_maps_model_points_back_to_the_screen() {
        let capture = Capture {
            target: "t".into(),
            ts: 0,
            space: "screen-points".into(),
            crop: [100.0, 50.0, 800.0, 600.0],
            scale: 2.0,
            redacted: false,
            derived_from: None,
        };
        // A 400x300 image sent for an 800x600-point crop: halfway is 400pt in.
        assert_eq!(capture.to_space((400, 300), (200.0, 150.0)), (500.0, 350.0));
        assert_eq!(capture.to_space((400, 300), (0.0, 0.0)), (100.0, 50.0));
    }
}
