use k256::schnorr::SigningKey;

type Result<T> = std::result::Result<T, TypesError>;

pub const SECP256K1: k256::Secp256k1 = k256::Secp256k1;

/// Error types for the types module
#[derive(Debug, thiserror::Error)]
pub enum TypesError {
    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Missing field: {0}")]
    MissingField(String),

    #[error("Invalid version: {0}")]
    InvalidVersion(i32),

    #[error("Other error: {0}")]
    Other(String),
}

#[derive(Clone)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes =
            hex::decode(s).map_err(|_| TypesError::InvalidFormat("Invalid hex".to_string()))?;
        if bytes.len() != 32 {
            return Err(TypesError::InvalidFormat("Invalid pubkey".to_string()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(PublicKey(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn to_string(&self) -> String {
        self.to_hex()
    }
}

pub struct SecretKey(pub [u8; 32]);

impl SecretKey {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes =
            hex::decode(s).map_err(|_| TypesError::InvalidFormat("Invalid hex".to_string()))?;
        if bytes.len() != 32 {
            return Err(TypesError::InvalidFormat("Invalid secret key".to_string()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(SecretKey(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn public_key(&self, _secp: &k256::Secp256k1) -> PublicKey {
        let signing_key = SigningKey::from_bytes(&self.0).unwrap();
        let verifying_key = signing_key.verifying_key();
        PublicKey(verifying_key.to_bytes().into())
    }

    pub fn display_secret(&self) -> String {
        self.to_hex()
    }
}

pub struct Keys {
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

impl Keys {
    pub fn new(secret_key: SecretKey) -> Self {
        let public_key = secret_key.public_key(&SECP256K1);
        Self {
            secret_key,
            public_key,
        }
    }

    pub fn parse(nsec: &str) -> Result<Self> {
        // Check if it starts with "nsec1" for bech32 format
        if nsec.starts_with("nsec1") {
            // For now, return error since bech32 is not implemented
            return Err(TypesError::InvalidFormat(
                "Bech32 nsec parsing not implemented".to_string(),
            ));
        }

        // Otherwise treat it as hex
        let secret_key = SecretKey::from_hex(nsec)?;
        Ok(Self::new(secret_key))
    }

    pub fn generate() -> Self {
        let signing_key = SigningKey::random(&mut k256::elliptic_curve::rand_core::OsRng);
        let secret_bytes: [u8; 32] = signing_key.to_bytes().into();
        Self::new(SecretKey(secret_bytes))
    }

    pub fn secret_key(&self) -> Result<&SecretKey> {
        Ok(&self.secret_key)
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.public_key.0)
    }
}
