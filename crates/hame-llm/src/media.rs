//! What a model can be shown besides text, and how images and documents
//! from the artifact store are put into a request.
//!
//! Every adapter goes through an [`Inliner`]: it decides per image whether
//! the bytes are sent, and when they are not, it says why in a text note
//! that names the artifact. An image is never dropped silently, its base64
//! never lands in a text block, and one image that cannot be sent never
//! fails the whole request.

use base64::Engine as _;
use hame_session::artifact::{ArtifactError, image_dimensions, sniff_mime};
use hame_session::{ArtifactRef, Block, Role};

use crate::Request;

/// The wire family a model is reached through, which decides how a model id
/// is read when its capabilities are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// The Anthropic Messages API, or a server that speaks it.
    Anthropic,
    /// OpenAI's own API (Responses or chat completions at api.openai.com).
    OpenAi,
    /// Another server speaking the chat-completions format: DeepSeek, a
    /// gateway, a local server.
    Compatible,
}

/// What a model accepts besides text, and the limits it accepts it under.
///
/// Adapters resolve one from the provider and model id when they are built
/// ([`ModelProfile::resolve`]); configuration can replace it with the
/// adapter's `with_profile`, e.g. to turn vision on for a model served under
/// a name the table does not know:
///
/// ```
/// use hame_llm::{ChatCompletions, ModelProfile, Provider};
/// let profile = ModelProfile {
///     vision: true,
///     ..ModelProfile::resolve(Provider::Compatible, "my-vl-model")
/// };
/// let llm = ChatCompletions::new("http://localhost:8000/v1", "k", "my-vl-model")
///     .with_profile(profile);
/// assert!(llm.profile().vision);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelProfile {
    /// Whether images may be sent.
    pub vision: bool,
    /// Whether PDF documents may be sent as documents.
    pub pdf: bool,
    /// The largest image sent, in raw bytes before base64.
    pub max_image_bytes: u64,
    /// The longest image edge sent, in pixels.
    pub max_image_edge: u32,
    /// The most images one request carries. Beyond it the oldest images
    /// become notes, so the most recent ones are what the model sees.
    pub max_images: u32,
}

/// The largest PDF sent inline: a 32 MB request limit holds about 24 MB of
/// base64-decoded bytes.
pub const MAX_PDF_BYTES: u64 = 24_000_000;

impl ModelProfile {
    /// A model that takes text only.
    pub const TEXT_ONLY: Self = Self {
        vision: false,
        pdf: false,
        max_image_bytes: 0,
        max_image_edge: 0,
        max_images: 0,
    };

    /// Claude models. The Messages API takes images up to 10 MB of base64
    /// (7.5 MB raw), 8000 px on a side, and up to 600 per request (100 for
    /// 200k-context models): platform.claude.com/docs/en/build-with-claude/vision
    /// ("Request limits"). 100 is the limit every model meets.
    const CLAUDE: Self = Self {
        vision: true,
        pdf: true,
        max_image_bytes: 7_500_000,
        max_image_edge: 8000,
        max_images: 100,
    };

    /// OpenAI vision models. `image_url` holds at most 20 MiB of data URL
    /// (about 15 MB raw) and a request up to 500 images; OpenAI documents no
    /// edge limit and resizes server-side, so the edge only bounds payloads.
    const OPENAI: Self = Self {
        vision: true,
        pdf: true,
        max_image_bytes: 15_000_000,
        max_image_edge: 8192,
        max_images: 500,
    };

    /// A vision model behind some other chat-completions server: limits
    /// unknown, so modest ones, and no PDFs (file parts are OpenAI's own).
    const COMPATIBLE_VISION: Self = Self {
        vision: true,
        pdf: false,
        max_image_bytes: 5_000_000,
        max_image_edge: 4096,
        max_images: 20,
    };

    /// The known capabilities of `model` reached through `provider`.
    ///
    /// Known families: Claude models take images and PDFs; OpenAI's
    /// gpt-4o, gpt-4.1, gpt-4.5, gpt-5, o1, o3 and o4-mini families take
    /// images (and PDFs through the Responses API); DeepSeek's chat models
    /// take text only. A model the table does not know is treated as text
    /// only, because sending an image to a model that cannot take one fails
    /// the whole request, while a text note only costs the picture.
    pub fn resolve(provider: Provider, model: &str) -> Self {
        let id = model.to_ascii_lowercase();
        // Gateways prefix the vendor: `openai/gpt-4o`, `anthropic/claude-…`.
        let bare = id.rsplit('/').next().unwrap_or(&id);
        if bare.contains("deepseek") {
            return Self::TEXT_ONLY;
        }
        let claude = bare.starts_with("claude") || bare.contains(".claude");
        let openai_vision = {
            let family = |prefix: &str| bare.starts_with(prefix);
            (family("gpt-4o")
                || family("chatgpt-4o")
                || family("gpt-4.1")
                || family("gpt-4.5")
                || family("gpt-4-turbo")
                || family("gpt-5")
                || family("o1")
                || family("o3")
                || family("o4")
                || family("computer-use"))
                && !["o1-mini", "o1-preview", "o3-mini"]
                    .iter()
                    .any(|text_only| bare.starts_with(text_only))
        };
        match provider {
            Provider::Anthropic if claude => Self::CLAUDE,
            Provider::Anthropic => Self::TEXT_ONLY,
            Provider::OpenAi if openai_vision => Self::OPENAI,
            Provider::OpenAi => Self::TEXT_ONLY,
            Provider::Compatible if claude => Self {
                pdf: false,
                ..Self::CLAUDE
            },
            Provider::Compatible if openai_vision => Self {
                pdf: false,
                ..Self::OPENAI
            },
            Provider::Compatible
                if bare.starts_with("gemini")
                    || bare.contains("-vl")
                    || bare.contains("vision") =>
            {
                Self::COMPATIBLE_VISION
            }
            Provider::Compatible => Self::TEXT_ONLY,
        }
    }
}

/// What one image or file block becomes in a request.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Media {
    Image {
        mime: String,
        /// Standard base64 of the bytes.
        data: String,
    },
    Pdf {
        data: String,
        name: Option<String>,
    },
    /// The block is described instead of sent.
    Note(String),
}

impl Media {
    pub(crate) fn data_url(mime: &str, data: &str) -> String {
        format!("data:{mime};base64,{data}")
    }
}

/// Image types every provider here accepts.
const SENDABLE: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Decides, block by block, what of a request's images and files is sent.
pub(crate) struct Inliner<'a> {
    profile: ModelProfile,
    request: &'a Request,
    /// The leading images that are over the request's image budget.
    skip: usize,
    seen: usize,
    edge: u32,
    pub(crate) images: u32,
    pub(crate) image_bytes: u64,
}

/// The image blocks of the request a provider is shown: those in user and
/// tool turns, including inside tool results.
fn image_count(request: &Request) -> usize {
    fn count(block: &Block) -> usize {
        match block {
            Block::Image { .. } => 1,
            Block::ToolResult { content, .. } => content.iter().map(count).sum(),
            _ => 0,
        }
    }
    request
        .messages
        .iter()
        .filter(|message| message.role != Role::Assistant)
        .flat_map(|message| &message.content)
        .map(count)
        .sum()
}

impl<'a> Inliner<'a> {
    pub(crate) fn new(profile: ModelProfile, request: &'a Request) -> Self {
        let total = image_count(request);
        let budget = profile.max_images as usize;
        Self {
            profile,
            request,
            skip: total.saturating_sub(budget),
            seen: 0,
            edge: profile.max_image_edge,
            images: 0,
            image_bytes: 0,
        }
    }

    /// Lowers the edge limit to `edge` when the request carries more than
    /// `threshold` images, for providers with a stricter many-image rule.
    pub(crate) fn many_images(mut self, threshold: usize, edge: u32) -> Self {
        let sent = image_count(self.request).min(self.profile.max_images as usize);
        if sent > threshold {
            self.edge = self.edge.min(edge);
        }
        self
    }

    pub(crate) fn image(&mut self, artifact: &ArtifactRef) -> Media {
        let index = self.seen;
        self.seen += 1;
        let not_sent = |reason: String| Media::Note(image_note(artifact, &reason));
        let profile = self.profile;
        if !profile.vision {
            return not_sent("this model does not take images".into());
        }
        if index < self.skip {
            return not_sent(format!(
                "only the last {} images of a request are sent",
                profile.max_images
            ));
        }
        if !SENDABLE.contains(&artifact.mime.as_str()) {
            return not_sent(format!(
                "{} is not an image type models take",
                artifact.mime
            ));
        }
        let over_bytes = |bytes: u64| {
            format!(
                "{bytes} bytes is over the {}-byte limit for one image",
                profile.max_image_bytes
            )
        };
        if artifact.bytes > profile.max_image_bytes {
            return not_sent(over_bytes(artifact.bytes));
        }
        let edge = self.edge;
        let over_edge = |(w, h): (u32, u32)| {
            (w.max(h) > edge).then(|| format!("{w}x{h} is over the {edge} px edge limit"))
        };
        if let (Some(w), Some(h)) = (artifact.width, artifact.height)
            && let Some(reason) = over_edge((w, h))
        {
            return not_sent(reason);
        }
        let bytes = match self.read(artifact) {
            Ok(bytes) => bytes,
            Err(reason) => return not_sent(reason),
        };
        if bytes.len() as u64 > profile.max_image_bytes {
            return not_sent(over_bytes(bytes.len() as u64));
        }
        if let Some(reason) = image_dimensions(&bytes).and_then(over_edge) {
            return not_sent(reason);
        }
        // The provider checks the declared type against the bytes; what
        // the bytes are wins.
        let Some(mime) = sniff_mime(&bytes).filter(|mime| mime.starts_with("image/")) else {
            return not_sent("its bytes are not a PNG, JPEG, GIF or WebP image".into());
        };
        self.images += 1;
        self.image_bytes += bytes.len() as u64;
        Media::Image {
            mime: mime.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        }
    }

    /// A stored file: a PDF goes as a document when the model takes them;
    /// anything else is described.
    pub(crate) fn file(&mut self, artifact: &ArtifactRef) -> Media {
        let described = Block::Artifact {
            artifact: artifact.clone(),
        }
        .as_text()
        .unwrap_or_default();
        if artifact.mime != "application/pdf" {
            return Media::Note(described);
        }
        let not_sent = |reason: String| {
            Media::Note(format!(
                "{}: not sent, {reason}]",
                described.trim_end_matches(']')
            ))
        };
        if !self.profile.pdf {
            return not_sent("this model does not take PDF documents".into());
        }
        if artifact.bytes > MAX_PDF_BYTES {
            return not_sent(format!(
                "it is over the {MAX_PDF_BYTES}-byte limit for a document"
            ));
        }
        let bytes = match self.read(artifact) {
            Ok(bytes) => bytes,
            Err(reason) => return not_sent(reason),
        };
        if !bytes.starts_with(b"%PDF-") {
            return not_sent("its bytes are not a PDF".into());
        }
        Media::Pdf {
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
            name: artifact.name.clone(),
        }
    }

    fn read(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, String> {
        let Some(source) = &self.request.artifacts else {
            return Err("no artifact store was given to this request".into());
        };
        source.read(artifact).map_err(|e| match e {
            ArtifactError::Missing(_) => "it is missing from the artifact store".into(),
            ArtifactError::Corrupt { .. } => "its stored bytes are corrupt".into(),
            other => format!("it could not be read: {other}"),
        })
    }
}

impl Inliner<'_> {
    /// What one block of a user turn or tool result becomes: an image or
    /// document, or text (its own, or a description).
    pub(crate) fn block(&mut self, block: &Block) -> Media {
        match block {
            Block::Image { artifact } => self.image(artifact),
            Block::Artifact { artifact } => self.file(artifact),
            other => Media::Note(text_form(other).unwrap_or_default()),
        }
    }
}

/// A run of text, or one image or document sent natively.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Piece {
    Text(String),
    /// Never a `Media::Note`. The label describes the block in text, for
    /// formats that have to say where the media went.
    Media(Media, String),
}

/// Splits blocks into text runs and natively sent media, in order. Text
/// blocks run together, as they always did; a description of a block goes
/// on a line of its own. Empty text is left out.
pub(crate) fn pieces(blocks: &[Block], inliner: &mut Inliner) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut last_was_text = true;
    for block in blocks {
        let plain = matches!(block, Block::Text { .. });
        match inliner.block(block) {
            Media::Note(piece) => {
                if piece.is_empty() {
                    continue;
                }
                if !text.is_empty() && !(plain && last_was_text) {
                    text.push('\n');
                }
                text.push_str(&piece);
                last_was_text = plain;
            }
            media => {
                if !text.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut text)));
                }
                out.push(Piece::Media(media, text_form(block).unwrap_or_default()));
                last_was_text = true;
            }
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// Whether any piece is sent natively.
pub(crate) fn has_media(pieces: &[Piece]) -> bool {
    pieces.iter().any(|piece| matches!(piece, Piece::Media(..)))
}

/// All the text of pieces that hold no media.
pub(crate) fn joined(pieces: Vec<Piece>) -> String {
    pieces
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Text(text) => Some(text),
            Piece::Media(..) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `[image image/png 64x64 1234 bytes, artifact 0123456789ab: not sent, <reason>]`
fn image_note(artifact: &ArtifactRef, reason: &str) -> String {
    let described = Block::Image {
        artifact: artifact.clone(),
    }
    .as_text()
    .unwrap_or_default();
    format!("{}: not sent, {reason}]", described.trim_end_matches(']'))
}

/// The text a block becomes where only text can go: `Data` as JSON,
/// `Unsupported` as its note. `None` for blocks that have no text form.
pub(crate) fn text_form(block: &Block) -> Option<String> {
    block.as_text().filter(|text| !text.is_empty())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use hame_session::{ArtifactSource, ArtifactStore, Message};
    use std::sync::Arc;

    /// A valid PNG of `width` x `height` pixels: the left half `left`, the
    /// right half `right` (RGB). Stored uncompressed, so no zlib is needed.
    pub(crate) fn png(width: u32, height: u32, left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for &b in bytes {
                crc ^= u32::from(b);
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xedb8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }
        // Each row: filter type 0 (none), then the pixels.
        let row: Vec<u8> = std::iter::once(0)
            .chain((0..width).flat_map(|x| if x < width / 2 { left } else { right }))
            .collect();
        let raw = row.repeat(height as usize);
        // zlib stream of stored deflate blocks.
        let mut zlib = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
        for (i, block) in blocks.iter().enumerate() {
            zlib.push(u8::from(i + 1 == blocks.len()));
            let len = block.len() as u16;
            zlib.extend_from_slice(&len.to_le_bytes());
            zlib.extend_from_slice(&(!len).to_le_bytes());
            zlib.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &raw {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        chunk(&mut out, b"IHDR", &header);
        chunk(&mut out, b"IDAT", &zlib);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    pub(crate) fn store() -> (tempfile::TempDir, ArtifactStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(dir.path().join("artifacts"));
        (dir, store)
    }

    /// A conversation with an image and a PDF in the user turn, and a tool
    /// result holding text, an image, an image missing from the store,
    /// structured data and a note.
    pub(crate) struct Fixture {
        pub _dir: tempfile::TempDir,
        pub request: Request,
        pub red: ArtifactRef,
        pub blue: ArtifactRef,
        pub gone: ArtifactRef,
    }

    pub(crate) fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    pub(crate) fn fixture() -> Fixture {
        let (dir, store) = store();
        let red = store
            .put(&png(8, 8, [255, 0, 0], [255, 0, 0]), "image/png")
            .unwrap();
        let blue = store
            .put(&png(8, 8, [0, 0, 255], [0, 0, 255]), "image/png")
            .unwrap();
        let mut gone = red.clone();
        gone.sha256 = "cd".repeat(32);
        let mut pdf = store.put(b"%PDF-1.4 x", "application/pdf").unwrap();
        pdf.name = Some("spec.pdf".into());
        let request = Request {
            messages: vec![
                Message {
                    role: Role::User,
                    content: vec![
                        Block::text("Compare these."),
                        Block::Image {
                            artifact: red.clone(),
                        },
                        Block::Artifact { artifact: pdf },
                    ],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![Block::ToolUse {
                        id: "t1".into(),
                        name: "shot".into(),
                        input: serde_json::json!({}),
                    }],
                },
                Message {
                    role: Role::Tool,
                    content: vec![Block::ToolResult {
                        call_id: "t1".into(),
                        content: vec![
                            Block::text("captured"),
                            Block::Image {
                                artifact: blue.clone(),
                            },
                            Block::Image {
                                artifact: gone.clone(),
                            },
                            Block::Data {
                                value: serde_json::json!({"ok": true}),
                            },
                            Block::Unsupported {
                                kind: "audio".into(),
                                note: "audio/wav, 10 bytes".into(),
                            },
                        ],
                        is_error: false,
                    }],
                },
            ],
            artifacts: Some(ArtifactSource(Arc::new(store))),
            ..Request::default()
        };
        Fixture {
            _dir: dir,
            request,
            red,
            blue,
            gone,
        }
    }

    fn request_with(blocks: Vec<Block>, store: Option<&ArtifactStore>) -> Request {
        Request {
            messages: vec![Message {
                role: Role::User,
                content: blocks,
            }],
            artifacts: store.map(|store| ArtifactSource(Arc::new(store.clone()))),
            ..Request::default()
        }
    }

    #[test]
    fn known_families_resolve_to_their_capabilities() {
        use Provider::*;
        assert!(ModelProfile::resolve(Anthropic, "claude-haiku-5-5").vision);
        assert!(ModelProfile::resolve(Anthropic, "claude-opus-4-5-20251101").pdf);
        assert!(!ModelProfile::resolve(Anthropic, "deepseek-chat").vision);
        for vision in [
            "gpt-4o-mini",
            "gpt-4.1",
            "gpt-5.2",
            "o3",
            "o4-mini",
            "openai/gpt-4o",
        ] {
            assert!(ModelProfile::resolve(OpenAi, vision).vision, "{vision}");
        }
        for text in ["gpt-3.5-turbo", "o3-mini", "o1-mini", "gpt-4"] {
            assert!(!ModelProfile::resolve(OpenAi, text).vision, "{text}");
        }
        assert_eq!(
            ModelProfile::resolve(Compatible, "deepseek-chat"),
            ModelProfile::TEXT_ONLY
        );
        assert_eq!(
            ModelProfile::resolve(Compatible, "deepseek-reasoner"),
            ModelProfile::TEXT_ONLY
        );
        assert_eq!(
            ModelProfile::resolve(Compatible, "some-local-model"),
            ModelProfile::TEXT_ONLY
        );
        let gateway = ModelProfile::resolve(Compatible, "anthropic/claude-sonnet-5-5");
        assert!(gateway.vision && !gateway.pdf);
        assert!(ModelProfile::resolve(Compatible, "qwen2.5-vl-72b").vision);
    }

    #[test]
    fn an_image_is_sent_or_described_with_the_reason() {
        let (_dir, store) = store();
        let red = store
            .put(&png(4, 4, [255, 0, 0], [255, 0, 0]), "image/png")
            .unwrap();
        let big = store.put(&png(64, 8, [0; 3], [0; 3]), "image/png").unwrap();
        let mut gone = red.clone();
        gone.sha256 = "ab".repeat(32);
        let blocks = vec![
            Block::Image {
                artifact: red.clone(),
            },
            Block::Image {
                artifact: big.clone(),
            },
            Block::Image {
                artifact: gone.clone(),
            },
        ];
        let request = request_with(blocks.clone(), Some(&store));
        let profile = ModelProfile {
            max_image_edge: 32,
            ..ModelProfile::CLAUDE
        };
        let mut inliner = Inliner::new(profile, &request);
        assert!(matches!(inliner.image(&red), Media::Image { mime, .. } if mime == "image/png"));
        let Media::Note(note) = inliner.image(&big) else {
            panic!("too wide");
        };
        assert!(note.contains("64x8 is over the 32 px edge limit"), "{note}");
        assert!(note.contains(big.short()), "{note}");
        let Media::Note(note) = inliner.image(&gone) else {
            panic!("missing");
        };
        assert!(note.contains("missing from the artifact store"), "{note}");
        assert!(note.contains(&gone.sha256[..12]), "{note}");
        assert_eq!(inliner.images, 1);
        assert_eq!(inliner.image_bytes, red.bytes);

        // No store, no vision, too many: each says so.
        let request = request_with(blocks.clone(), None);
        let Media::Note(note) = Inliner::new(ModelProfile::CLAUDE, &request).image(&red) else {
            panic!();
        };
        assert!(note.contains("no artifact store"), "{note}");
        let Media::Note(note) = Inliner::new(ModelProfile::TEXT_ONLY, &request).image(&red) else {
            panic!();
        };
        assert!(note.contains("does not take images"), "{note}");
        assert!(!note.contains("iVBOR"), "never base64 in text");
        let request = request_with(blocks, Some(&store));
        let mut two = Inliner::new(
            ModelProfile {
                max_images: 2,
                ..ModelProfile::CLAUDE
            },
            &request,
        );
        let Media::Note(note) = two.image(&red) else {
            panic!("the oldest image is over the budget");
        };
        assert!(note.contains("only the last 2 images"), "{note}");
        assert!(matches!(two.image(&big), Media::Image { .. }));
    }

    #[test]
    fn a_pdf_goes_as_a_document_only_where_the_model_takes_them() {
        let (_dir, store) = store();
        let mut pdf = store.put(b"%PDF-1.4 tiny", "application/pdf").unwrap();
        pdf.name = Some("report.pdf".into());
        let request = request_with(vec![], Some(&store));
        assert!(matches!(
            Inliner::new(ModelProfile::CLAUDE, &request).file(&pdf),
            Media::Pdf { name: Some(name), .. } if name == "report.pdf"
        ));
        let Media::Note(note) = Inliner::new(ModelProfile::TEXT_ONLY, &request).file(&pdf) else {
            panic!();
        };
        assert!(note.contains("does not take PDF"), "{note}");
        let csv = store.put(b"a,b", "text/csv").unwrap();
        let Media::Note(note) = Inliner::new(ModelProfile::CLAUDE, &request).file(&csv) else {
            panic!();
        };
        assert!(note.starts_with("[file text/csv"), "{note}");
    }

    #[test]
    fn the_test_png_is_a_png() {
        let bytes = png(64, 64, [255, 0, 0], [0, 0, 255]);
        assert_eq!(sniff_mime(&bytes), Some("image/png"));
        assert_eq!(image_dimensions(&bytes), Some((64, 64)));
    }
}
