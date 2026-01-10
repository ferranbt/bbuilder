use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::SigningKey;
use k256::ecdsa::SigningKey as kSigningKey;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

#[derive(Serialize, Deserialize)]
pub struct NodeKey {
    pub priv_key: PrivateKey,
}

impl NodeKey {
    pub fn serialize(&self) -> eyre::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

#[derive(Serialize, Deserialize)]
pub struct PrivateKey {
    #[serde(rename = "type")]
    pub key_type: String,
    pub value: String,
}

#[derive(Serialize, Deserialize)]
pub struct ValidatorKey {
    pub address: String,
    pub pub_key: PublicKey,
    pub priv_key: PrivateKey,
}

impl ValidatorKey {
    pub fn serialize(&self) -> eyre::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[derive(Serialize, Deserialize)]
pub struct PublicKey {
    #[serde(rename = "type")]
    pub key_type: String,
    pub value: String,
}

pub fn generate_tendermint_key() -> NodeKey {
    let signing_key = SigningKey::generate(&mut OsRng);
    let private_bytes = signing_key.to_bytes();
    let public_bytes = signing_key.verifying_key().to_bytes();

    let mut combined = Vec::with_capacity(64);
    combined.extend_from_slice(&private_bytes);
    combined.extend_from_slice(&public_bytes);

    let encoded = general_purpose::STANDARD.encode(&combined);

    NodeKey {
        priv_key: PrivateKey {
            key_type: "tendermint/PrivKeyEd25519".to_string(),
            value: encoded,
        },
    }
}

pub fn generate_cometbft_key() -> ValidatorKey {
    // Generate secp256k1 key pair
    let signing_key = kSigningKey::random(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Get private key bytes (32 bytes)
    let private_bytes = signing_key.to_bytes();
    let priv_base64 = general_purpose::STANDARD.encode(&private_bytes);

    // Get public key bytes (uncompressed, 65 bytes)
    let public_bytes = verifying_key.to_encoded_point(false);
    let pub_base64 = general_purpose::STANDARD.encode(public_bytes.as_bytes());

    // Generate address: first 20 bytes of keccak256(public_key)
    let hash = Keccak256::digest(&public_bytes.as_bytes()[1..]); // Skip the 0x04 prefix
    let address_bytes = &hash[12..]; // Take last 20 bytes
    let address = hex::encode_upper(address_bytes);

    ValidatorKey {
        address,
        pub_key: PublicKey {
            key_type: "cometbft/PubKeySecp256k1eth".to_string(),
            value: pub_base64,
        },
        priv_key: PrivateKey {
            key_type: "cometbft/PrivKeySecp256k1eth".to_string(),
            value: priv_base64,
        },
    }
}
