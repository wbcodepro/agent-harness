//! Pairing (§4): the QR offer, the client's HMAC proof, the short
//! authentication string both screens show, and the host's signed grant.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::SyncError;
use crate::keys::{Identity, PublicIdentity, b64, random32, unb64, unb64_32, verify_signature};

pub const SCHEME: &str = "hame-pair:v1?";
/// How long a pairing offer is valid.
pub const OFFER_SECONDS: u64 = 600;
pub const GRANT_CONTEXT: &[u8] = b"hame-sync v1 grant";
/// The `kind` field of a published grant list.
pub const GRANT_LIST_KIND: &str = "hame-sync/grants";

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the host's QR code carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingOffer {
    pub relay: String,
    pub host: String,
    pub host_keys: PublicIdentity,
    pub secret: [u8; 32],
    pub expires: u64,
}

impl PairingOffer {
    /// A fresh offer with a random secret, valid for ten minutes.
    pub fn new(relay: &str, host: &str, host_keys: PublicIdentity) -> Self {
        Self {
            relay: relay.into(),
            host: host.into(),
            host_keys,
            secret: random32(),
            expires: now_unix() + OFFER_SECONDS,
        }
    }

    /// `hame-pair:v1?relay=…&host=…&ed=…&x=…&secret=…&exp=…`, the query
    /// part form-encoded.
    pub fn to_uri(&self) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("relay", &self.relay)
            .append_pair("host", &self.host)
            .append_pair("ed", &b64(&self.host_keys.ed))
            .append_pair("x", &b64(&self.host_keys.x))
            .append_pair("secret", &b64(&self.secret))
            .append_pair("exp", &self.expires.to_string())
            .finish();
        format!("{SCHEME}{query}")
    }

    pub fn parse(uri: &str) -> Result<Self, SyncError> {
        let query = uri
            .strip_prefix(SCHEME)
            .ok_or(SyncError::Malformed("not a hame pairing code"))?;
        let mut fields = std::collections::HashMap::new();
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
            {
                return Err(SyncError::Malformed("repeated field in pairing code"));
            }
        }
        let field = |name: &'static str| {
            fields
                .get(name)
                .cloned()
                .ok_or(SyncError::Malformed("pairing code misses a field"))
        };
        let relay = field("relay")?;
        crate::relay::check_relay_url(&relay)?;
        let host = field("host")?;
        if !crate::text::valid_id(&host) {
            return Err(SyncError::Malformed("bad host id in pairing code"));
        }
        Ok(Self {
            relay,
            host,
            host_keys: PublicIdentity {
                ed: unb64_32(&field("ed")?)?,
                x: unb64_32(&field("x")?)?,
            },
            secret: unb64_32(&field("secret")?)?,
            expires: field("exp")?
                .parse()
                .map_err(|_| SyncError::Malformed("bad expiry"))?,
        })
    }

    pub fn expired(&self, now: u64) -> bool {
        now >= self.expires
    }
}

/// `HMAC-SHA256(secret, client_ed ‖ client_x)`: proves the client read the
/// QR code.
pub fn pairing_proof(secret: &[u8; 32], client: &PublicIdentity) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("any key length");
    mac.update(&client.ed);
    mac.update(&client.x);
    mac.finalize().into_bytes().into()
}

/// Checks a proof in constant time.
pub fn check_pairing_proof(
    secret: &[u8; 32],
    client: &PublicIdentity,
    proof: &[u8],
) -> Result<(), SyncError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("any key length");
    mac.update(&client.ed);
    mac.update(&client.x);
    mac.verify_slice(proof)
        .map_err(|_| SyncError::Pairing("the pairing proof does not match".into()))
}

/// Eight digits from
/// `SHA256("hame-sas-v2" ‖ secret ‖ host_ed ‖ client_ed ‖ host_nonce)`: the
/// first eight bytes as a big-endian number, modulo 10^8, zero-padded. The
/// host's nonce is revealed only after the device sent its keys (in a
/// `pair_challenge`), so no one can search for keys that give chosen digits.
pub fn sas(
    secret: &[u8; 32],
    host_ed: &[u8; 32],
    client_ed: &[u8; 32],
    host_nonce: &[u8; 32],
) -> String {
    let digest = Sha256::new()
        .chain_update(b"hame-sas-v2")
        .chain_update(secret)
        .chain_update(host_ed)
        .chain_update(client_ed)
        .chain_update(host_nonce)
        .finalize();
    let number = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    format!("{:08}", number % 100_000_000)
}

/// The host's statement that it accepts a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub v: u32,
    pub host: String,
    pub device: String,
    pub ed: String,
    pub x: String,
    pub label: String,
    pub issued: u64,
}

/// A grant as sent: its exact bytes and the host's signature over them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedGrant {
    pub grant: String,
    pub sig: String,
}

fn sign_document(host: &Identity, context: &[u8], bytes: &[u8]) -> String {
    let mut message = context.to_vec();
    message.extend_from_slice(bytes);
    b64(&host.sign(&message))
}

fn verify_document(
    host_ed: &[u8; 32],
    context: &[u8],
    bytes: &[u8],
    signature: &str,
) -> Result<(), SyncError> {
    let signature: [u8; 64] = unb64(signature)?
        .try_into()
        .map_err(|_| SyncError::BadSignature)?;
    let mut message = context.to_vec();
    message.extend_from_slice(bytes);
    verify_signature(host_ed, &message, &signature)
}

impl Grant {
    pub fn new(host: &str, device: &str, keys: &PublicIdentity, label: &str) -> Self {
        Self {
            v: 1,
            host: host.into(),
            device: device.into(),
            ed: b64(&keys.ed),
            x: b64(&keys.x),
            label: label.into(),
            issued: now_unix(),
        }
    }

    /// Signs with `"hame-sync v1 grant" ‖ grant JSON bytes`.
    pub fn sign(&self, host: &Identity) -> SignedGrant {
        let bytes = serde_json::to_vec(self).expect("a grant serialises");
        SignedGrant {
            sig: sign_document(host, GRANT_CONTEXT, &bytes),
            grant: b64(&bytes),
        }
    }
}

impl SignedGrant {
    pub fn verify(&self, host_ed: &[u8; 32]) -> Result<Grant, SyncError> {
        let bytes = unb64(&self.grant)?;
        verify_document(host_ed, GRANT_CONTEXT, &bytes, &self.sig)?;
        serde_json::from_slice(&bytes).map_err(|_| SyncError::Malformed("bad grant"))
    }
}

/// One accepted device in the list a host publishes to the relay
/// (`POST /v1/hosts/{id}/grants`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantEntry {
    pub device: String,
    pub ed: String,
    pub x: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantList {
    /// Always [`GRANT_LIST_KIND`]: the document names what it is, since the
    /// relay checks a plain signature over its bytes.
    pub kind: String,
    pub v: u32,
    pub host: String,
    /// Increases with every list the host publishes, so an older one
    /// cannot be passed off as current.
    pub version: u64,
    pub devices: Vec<GrantEntry>,
    pub issued: u64,
}

/// The body of `POST /v1/hosts/{id}/grants` (and what
/// `GET /v1/hosts/{id}/grants` returns): a strictly increasing `version`,
/// the document bytes and the host's Ed25519 signature over
/// `"hame-grants:v1\n{host_id}\n{version}\n" ‖ document`, both base64url.
/// The relay checks the signature with the host's registered key and the
/// version against the stored one before storing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedGrantList {
    pub version: u64,
    pub document: String,
    pub signature: String,
}

/// What the host signs for a grant list (the relay's format).
pub fn grant_list_message(host: &str, version: u64, document: &[u8]) -> Vec<u8> {
    let mut message = format!("hame-grants:v1\n{host}\n{version}\n").into_bytes();
    message.extend_from_slice(document);
    message
}

impl GrantList {
    pub fn new(host: &str, version: u64, devices: Vec<GrantEntry>) -> Self {
        Self {
            kind: GRANT_LIST_KIND.into(),
            v: 1,
            host: host.into(),
            version,
            devices,
            issued: now_unix(),
        }
    }

    /// Signs in the relay's format, see [`SignedGrantList`].
    pub fn sign(&self, host: &Identity) -> SignedGrantList {
        let bytes = serde_json::to_vec(self).expect("a grant list serialises");
        let message = grant_list_message(&self.host, self.version, &bytes);
        SignedGrantList {
            version: self.version,
            signature: b64(&host.sign(&message)),
            document: b64(&bytes),
        }
    }
}

impl SignedGrantList {
    /// Verifies a list published by the host `host_id`.
    pub fn verify(&self, host_id: &str, host_ed: &[u8; 32]) -> Result<GrantList, SyncError> {
        let bytes = unb64(&self.document)?;
        let message = grant_list_message(host_id, self.version, &bytes);
        verify_document(host_ed, b"", &message, &self.signature)?;
        let list: GrantList =
            serde_json::from_slice(&bytes).map_err(|_| SyncError::Malformed("bad grant list"))?;
        if list.kind != GRANT_LIST_KIND || list.host != host_id || list.version != self.version {
            return Err(SyncError::Malformed("not this host's grant list"));
        }
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_round_trips_through_its_uri() {
        let host = Identity::generate();
        let offer = PairingOffer::new("https://relay.example/x?y=1&z", "host-1", host.public());
        let uri = offer.to_uri();
        assert!(uri.starts_with("hame-pair:v1?relay=https%3A%2F%2Frelay.example"));
        assert_eq!(PairingOffer::parse(&uri).unwrap(), offer);
        assert!(PairingOffer::parse("hame-pair:v2?relay=x").is_err());
        assert!(PairingOffer::parse(&format!("{uri}&exp=1")).is_err());
        let plain = PairingOffer::new("http://relay.example", "h", host.public());
        assert!(
            PairingOffer::parse(&plain.to_uri()).is_err(),
            "no plain http"
        );
        let local = PairingOffer::new("http://127.0.0.1:8080", "h", host.public());
        assert!(PairingOffer::parse(&local.to_uri()).is_ok());
        let spaced = PairingOffer::new("https://r", "host 1", host.public());
        assert!(
            PairingOffer::parse(&spaced.to_uri()).is_err(),
            "bad host id"
        );
        assert!(!offer.expired(now_unix()));
        assert!(offer.expired(offer.expires));
    }

    #[test]
    fn proofs_and_sas_bind_the_keys() {
        let secret = [7u8; 32];
        let client = Identity::generate().public();
        let other = Identity::generate().public();
        let proof = pairing_proof(&secret, &client);
        check_pairing_proof(&secret, &client, &proof).unwrap();
        assert!(check_pairing_proof(&secret, &other, &proof).is_err());
        assert!(check_pairing_proof(&[8u8; 32], &client, &proof).is_err());
        let code = sas(&secret, &[1; 32], &client.ed, &[9; 32]);
        assert_eq!(code.len(), 8);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        assert_ne!(code, sas(&secret, &[1; 32], &other.ed, &[9; 32]));
        assert_ne!(code, sas(&secret, &[1; 32], &client.ed, &[8; 32]));
    }

    #[test]
    fn grants_verify_only_with_the_host_key() {
        let host = Identity::generate();
        let device = Identity::generate().public();
        let signed = Grant::new("h", "d", &device, "Phone").sign(&host);
        let grant = signed.verify(&host.public().ed).unwrap();
        assert_eq!(grant.device, "d");
        assert!(signed.verify(&Identity::generate().public().ed).is_err());
        let list = GrantList::new("h", 3, vec![]);
        assert_eq!(
            list.sign(&host).verify("h", &host.public().ed).unwrap(),
            list
        );
        assert!(list.sign(&host).verify("other", &host.public().ed).is_err());
    }
}
