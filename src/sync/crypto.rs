//! Примитивы протокола: P-256 ECDH, AES-256-GCM, SHA-256, HMAC и HKDF.

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use aws_lc_rs::agreement::{self, ECDH_P256, PrivateKey, UnparsedPublicKey};
use aws_lc_rs::constant_time::verify_slices_are_equal;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use super::SyncError;

/// Длина открытого ключа P-256 без сжатия.
pub const PUBLIC_KEY_LEN: usize = 65;
/// Длина тега AES-GCM.
const TAG_LEN: usize = 16;

/// `N` случайных байт.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], SyncError> {
    let mut out = [0u8; N];
    aws_lc_rs::rand::fill(&mut out).map_err(|_| SyncError::Crypto)?;
    Ok(out)
}

/// Сравнение за постоянное время.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    verify_slices_are_equal(a, b).is_ok()
}

/// Эфемерный ключ P-256.
pub struct EphemeralKey {
    private: PrivateKey,
    public: [u8; PUBLIC_KEY_LEN],
}

impl EphemeralKey {
    pub fn generate() -> Result<Self, SyncError> {
        let private = PrivateKey::generate(&ECDH_P256).map_err(|_| SyncError::Crypto)?;
        Self::from_private(private)
    }

    /// Из фиксированного скаляра (32 байта): для тестовых векторов.
    pub fn from_scalar(scalar: &[u8]) -> Result<Self, SyncError> {
        let private =
            PrivateKey::from_private_key(&ECDH_P256, scalar).map_err(|_| SyncError::Crypto)?;
        Self::from_private(private)
    }

    fn from_private(private: PrivateKey) -> Result<Self, SyncError> {
        let public = private
            .compute_public_key()
            .map_err(|_| SyncError::Crypto)?;
        let public: [u8; PUBLIC_KEY_LEN] =
            public.as_ref().try_into().map_err(|_| SyncError::Crypto)?;
        Ok(Self { private, public })
    }

    pub fn public(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.public
    }

    /// Общий секрет `Z`: координата X, 32 байта.
    pub fn agree(&self, peer_public: &[u8]) -> Result<[u8; 32], SyncError> {
        if peer_public.len() != PUBLIC_KEY_LEN || peer_public.first() != Some(&4) {
            return Err(SyncError::Protocol("неверный открытый ключ"));
        }
        let peer = UnparsedPublicKey::new(&ECDH_P256, peer_public);
        agreement::agree(&self.private, peer, SyncError::Crypto, |z| {
            z.try_into().map_err(|_| SyncError::Crypto)
        })
    }
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn hmac_new(key: &[u8]) -> Result<Hmac<Sha256>, SyncError> {
    Hmac::<Sha256>::new_from_slice(key).map_err(|_| SyncError::Crypto)
}

pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], SyncError> {
    let mut mac = hmac_new(key)?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

/// HKDF-Extract (RFC 5869).
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> Result<[u8; 32], SyncError> {
    hmac_sha256(salt, &[ikm])
}

/// HKDF-Expand (RFC 5869) на `N` байт (не больше 255 * 32).
pub fn hkdf_expand<const N: usize>(prk: &[u8; 32], info: &[u8]) -> Result<[u8; N], SyncError> {
    let mut okm: Vec<u8> = Vec::with_capacity(N);
    let mut previous: Vec<u8> = Vec::new();
    let mut counter: u8 = 0;
    while okm.len() < N {
        counter = counter.checked_add(1).ok_or(SyncError::Crypto)?;
        let block = hmac_sha256(prk, &[&previous, info, &[counter]])?;
        okm.extend_from_slice(&block);
        previous = block.to_vec();
    }
    okm.truncate(N);
    okm.try_into().map_err(|_| SyncError::Crypto)
}

/// Ключ AES-256-GCM и счётчик кадров одного направления.
struct Cipher {
    key: LessSafeKey,
    counter: u64,
}

impl Cipher {
    fn new(key: &[u8; 32]) -> Result<Self, SyncError> {
        let key = UnboundKey::new(&AES_256_GCM, key).map_err(|_| SyncError::Crypto)?;
        Ok(Self {
            key: LessSafeKey::new(key),
            counter: 0,
        })
    }

    /// Nonce: 4 нулевых байта и счётчик `u64` big-endian.
    fn nonce(&self) -> Result<Nonce, SyncError> {
        if self.counter >= 1 << 32 {
            return Err(SyncError::CounterExhausted);
        }
        let mut raw = [0u8; NONCE_LEN];
        if let Some(tail) = raw.get_mut(4..) {
            tail.copy_from_slice(&self.counter.to_be_bytes());
        }
        Ok(Nonce::assume_unique_for_key(raw))
    }
}

/// Шифрует кадры одного направления.
pub struct SealKey(Cipher);

impl SealKey {
    pub fn new(key: &[u8; 32]) -> Result<Self, SyncError> {
        Cipher::new(key).map(Self)
    }

    /// Шифротекст с тегом; счётчик растёт.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, SyncError> {
        let nonce = self.0.nonce()?;
        let mut buf = plaintext.to_vec();
        self.0
            .key
            .seal_in_place_append_tag(nonce, Aad::empty(), &mut buf)
            .map_err(|_| SyncError::Crypto)?;
        self.0.counter += 1;
        Ok(buf)
    }
}

/// Расшифровывает кадры одного направления; порядок и неповторяемость
/// держит счётчик.
pub struct OpenKey(Cipher);

impl OpenKey {
    pub fn new(key: &[u8; 32]) -> Result<Self, SyncError> {
        Cipher::new(key).map(Self)
    }

    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>, SyncError> {
        if frame.len() < TAG_LEN {
            return Err(SyncError::Decrypt);
        }
        let nonce = self.0.nonce()?;
        let mut buf = frame.to_vec();
        let len = self
            .0
            .key
            .open_in_place(nonce, Aad::empty(), &mut buf)
            .map_err(|_| SyncError::Decrypt)?
            .len();
        buf.truncate(len);
        self.0.counter += 1;
        Ok(buf)
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn unhex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::{hex, unhex};
    use super::*;

    #[test]
    fn hkdf_rfc5869_case_1() {
        let ikm = [0x0b; 22];
        let salt = unhex("000102030405060708090a0b0c");
        let info = unhex("f0f1f2f3f4f5f6f7f8f9");
        let prk = hkdf_extract(&salt, &ikm).unwrap();
        assert_eq!(
            hex(&prk),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
        );
        let okm: [u8; 42] = hkdf_expand(&prk, &info).unwrap();
        assert_eq!(
            hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    #[test]
    fn ecdh_agrees_both_ways() {
        let a = EphemeralKey::generate().unwrap();
        let b = EphemeralKey::generate().unwrap();
        assert_eq!(a.public().len(), 65);
        assert_eq!(a.agree(b.public()).unwrap(), b.agree(a.public()).unwrap());
        assert!(a.agree(&[4; 65]).is_err());
        assert!(a.agree(&[1; 10]).is_err());
    }

    #[test]
    fn cipher_roundtrip_and_tamper() {
        let key = [9u8; 32];
        let mut seal = SealKey::new(&key).unwrap();
        let mut open = OpenKey::new(&key).unwrap();
        let first = seal.seal(b"one").unwrap();
        let second = seal.seal(b"two").unwrap();
        assert_eq!(first.len(), 3 + 16);
        assert_eq!(open.open(&first).unwrap(), b"one");
        // Повтор первого кадра: счётчик ушёл вперёд.
        assert!(matches!(open.open(&first), Err(SyncError::Decrypt)));
        assert_eq!(open.open(&second).unwrap(), b"two");

        let mut seal = SealKey::new(&key).unwrap();
        let mut open = OpenKey::new(&key).unwrap();
        let mut frame = seal.seal(b"data").unwrap();
        if let Some(byte) = frame.get_mut(1) {
            *byte ^= 1;
        }
        assert!(matches!(open.open(&frame), Err(SyncError::Decrypt)));
        assert!(matches!(open.open(&[1, 2]), Err(SyncError::Decrypt)));
    }

    #[test]
    fn skipped_frame_is_an_error() {
        let key = [3u8; 32];
        let mut seal = SealKey::new(&key).unwrap();
        let mut open = OpenKey::new(&key).unwrap();
        let _lost = seal.seal(b"a").unwrap();
        let next = seal.seal(b"b").unwrap();
        assert!(open.open(&next).is_err());
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
