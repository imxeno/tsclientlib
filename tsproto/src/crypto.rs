//! This module contains cryptography related code.
use std::{cmp, fmt, str};

use arrayref::array_ref;
use base64;
use num_bigint::{BigInt, Sign};
use ring::digest;
use simple_asn1::{ASN1Block, ASN1Class};

use curve25519_dalek::constants;
use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
use curve25519_dalek::scalar::Scalar;
use openssl::bn::{BigNum, BigNumContext};
use openssl::derive::Deriver;
use openssl::ec::{self, EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private, Public};
use openssl::sign::{Signer, Verifier};
use openssl::symm::{self, Cipher};

use crate::{Error, Result};

pub enum KeyType {
	Public,
	Private,
}

/// A public ecc key.
///
/// The curve of this key is P-256, or PRIME256v1 as it is called by openssl.
#[derive(Clone)]
pub struct EccKeyPubP256(pub EcKey<Public>);
/// A private ecc key.
///
/// The curve of this key is P-256, or PRIME256v1 as it is called by openssl.
#[derive(Clone)]
pub struct EccKeyPrivP256(pub EcKey<Private>);

/// A public ecc key.
///
/// The curve of this key is Ed25519.
#[derive(Clone)]
pub struct EccKeyPubEd25519(pub CompressedEdwardsY);
/// A private ecc key.
///
/// The curve of this key is Ed25519.
#[derive(Clone)]
pub struct EccKeyPrivEd25519(pub Scalar);

impl fmt::Debug for EccKeyPubP256 {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "EccKeyPubP256({})", self.to_ts().unwrap())
	}
}

impl fmt::Debug for EccKeyPrivP256 {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "EccKeyPrivP256({})", base64::encode(&self.to_short()))
	}
}

impl fmt::Debug for EccKeyPubEd25519 {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "EccKeyPubEd25519({})", self.to_base64())
	}
}

impl fmt::Debug for EccKeyPrivEd25519 {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "EccKeyPrivEd25519({})", self.to_base64())
	}
}

impl EccKeyPubP256 {
	/// From base64 encoded tomcrypt key.
	pub fn from_ts(data: &str) -> Result<Self> {
		Self::from_tomcrypt(&base64::decode(data)?)
	}

	/// Decodes the public key from an ASN.1 DER object how tomcrypt stores it.
	///
	/// The format is:
	/// - `BitString` where the first bit is 1 if the private key is contained
	/// - `Integer`: The key size (32)
	/// - `Integer`: X coordinate of the public key
	/// - `Integer`: Y coordinate of the public key
	pub fn from_tomcrypt(data: &[u8]) -> Result<Self> {
		let blocks = ::simple_asn1::from_der(data)?;
		if blocks.len() != 1 {
			return Err(format_err!("More than one ASN.1 block").into());
		}
		if let ASN1Block::Sequence(_, blocks) = &blocks[0] {
			if let Some(ASN1Block::BitString(_, len, content)) = blocks.get(0) {
				if *len != 1 || content[0] & 0x80 != 0 {
					return Err(format_err!(
						"Expected a public key, not a private key"
					)
					.into());
				}
				if let (
					Some(ASN1Block::Integer(_, x)),
					Some(ASN1Block::Integer(_, y)),
				) = (blocks.get(2), blocks.get(3))
				{
					let x = BigNum::from_slice(&x.to_bytes_be().1)?;
					let y = BigNum::from_slice(&y.to_bytes_be().1)?;

					let group =
						EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
					let k = EcKey::from_public_key_affine_coordinates(
						&group, &x, &y,
					)?;
					Ok(EccKeyPubP256(k))
				} else {
					return Err(format_err!("Public key not found").into());
				}
			} else {
				return Err(format_err!("Expected a bitstring").into());
			}
		} else {
			return Err(format_err!("Expected a sequence").into());
		}
	}

	/// Convert to base64 encoded public tomcrypt key.
	pub fn to_ts(&self) -> Result<String> {
		Ok(base64::encode(&self.to_tomcrypt()?))
	}

	pub fn to_tomcrypt(&self) -> Result<Vec<u8>> {
		let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
		let mut ctx = BigNumContext::new()?;
		let pubkey_bin = self.0.public_key().to_bytes(
			&group,
			ec::PointConversionForm::UNCOMPRESSED,
			&mut ctx,
		)?;
		let pub_len = (pubkey_bin.len() - 1) / 2;
		let pubkey_x =
			BigInt::from_bytes_be(Sign::Plus, &pubkey_bin[1..=pub_len]);
		let pubkey_y =
			BigInt::from_bytes_be(Sign::Plus, &pubkey_bin[1 + pub_len..]);

		Ok(::simple_asn1::to_der(&ASN1Block::Sequence(
			0,
			vec![
				ASN1Block::BitString(0, 1, vec![0]),
				ASN1Block::Integer(0, 32.into()),
				ASN1Block::Integer(0, pubkey_x),
				ASN1Block::Integer(0, pubkey_y),
			],
		))?)
	}

	/// Compute the uid of this key.
	///
	/// Uid = base64(sha1(ts encoded key))
	pub fn get_uid(&self) -> Result<String> {
		let hash = digest::digest(&digest::SHA1, self.to_ts()?.as_bytes());
		Ok(base64::encode(&hash))
	}

	pub fn verify(self, data: &[u8], signature: &[u8]) -> Result<()> {
		let pkey = PKey::from_ec_key(self.0)?;
		let mut verifier = Verifier::new(MessageDigest::sha256(), &pkey)?;

		// Data
		verifier.update(data)?;
		let res = verifier.verify(signature)?;
		if res {
			Ok(())
		} else {
			Err(Error::WrongSignature)
		}
	}
}

impl EccKeyPrivP256 {
	/// Create a new key key pair.
	pub fn create() -> Result<Self> {
		let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
		Ok(EccKeyPrivP256(EcKey::generate(&group)?))
	}

	/// Try to import the key from any of the known formats.
	pub fn import(data: &[u8]) -> Result<Self> {
		if let Ok(s) = str::from_utf8(data) {
			if let Ok(r) = Self::import_str(s) {
				return Ok(r);
			}
		}
		if let Ok(r) = Self::from_tomcrypt(data) {
			return Ok(r);
		}
		if let Ok(r) = Self::from_short(data) {
			return Ok(r);
		}
		Err(format_err!("Any known methods to decode the key failed").into())
	}

	/// Try to import the key from any of the known formats.
	pub fn import_str(s: &str) -> Result<Self> {
		if let Ok(r) = base64::decode(s) {
			if let Ok(r) = Self::import(&r) {
				return Ok(r);
			}
		}
		if let Ok(r) = Self::from_ts_obfuscated(s) {
			return Ok(r);
		}
		Err(format_err!("Any known methods to decode the key failed").into())
	}

	/// The shortest format of a private key.
	///
	/// This is just the `BigNum` of the private key.
	pub fn from_short<V: Into<Vec<u8>>>(data: V) -> Result<Self> {
		// Convert to openssl format
		let der = ::simple_asn1::to_der(&ASN1Block::Sequence(
			0,
			vec![
				// Version
				ASN1Block::Integer(0, 1.into()),
				// Private key
				ASN1Block::OctetString(0, data.into()),
				// Parameters
				ASN1Block::Explicit(
					ASN1Class::ContextSpecific,
					0,
					0u8.into(),
					Box::new(ASN1Block::ObjectIdentifier(
						0,
						::simple_asn1::OID::new(vec![
							1u8.into(),
							2u8.into(),
							840u16.into(),
							10045u16.into(),
							3u8.into(),
							1u8.into(),
							7u8.into(),
						]),
					)),
				),
			],
		))?;

		let k = EcKey::private_key_from_der(&der)?;
		Ok(EccKeyPrivP256(k))
	}

	/// The shortest format of a private key.
	///
	/// This is just the `BigNum` of the private key.
	pub fn to_short(&self) -> Vec<u8> { self.0.private_key().to_vec() }

	/// From base64 encoded tomcrypt key.
	pub fn from_ts(data: &str) -> Result<Self> {
		Self::from_tomcrypt(&base64::decode(data)?)
	}

	/// From the key representation which is used to store identities in the
	/// TeamSpeak configuration file.
	///
	/// Format: Offset for identity level || 'V' || obfuscated key
	///
	/// This function takes only the obfuscated key without the level.
	///
	/// Thanks to landave, who put
	/// [his deobfuscate code](https://github.com/landave/TSIdentityTool)
	/// under the MIT license.
	pub fn from_ts_obfuscated(data: &str) -> Result<Self> {
		let mut data = base64::decode(data)?;
		if data.len() < 20 {
			return Err(
				format_err!("Not a known obfuscated TeamSpeak key").into()
			);
		}
		// Hash everything until the first 0 byte, starting after the first 20
		// bytes.
		let pos = data[20..]
			.iter()
			.position(|b| *b == b'\0')
			.unwrap_or(data.len() - 20);
		let hash = digest::digest(&digest::SHA1, &data[20..20 + pos]);
		let hash = hash.as_ref();
		// Xor first 20 bytes of data with the hash
		for i in 0..20 {
			data[i] ^= hash[i];
		}

		// Xor first 100 bytes with a static value
		#[allow(clippy::needless_range_loop)]
		for i in 0..cmp::min(data.len(), 100) {
			data[i] ^= crate::IDENTITY_OBFUSCATION[i];
		}
		Self::from_ts(str::from_utf8(&data)?)
	}

	/// Decodes the private key from an ASN.1 DER object how tomcrypt stores it.
	///
	/// The format is:
	/// - `BitString` where the first bit is 1 if the private key is contained
	/// - `Integer`: The key size (32)
	/// - `Integer`: X coordinate of the public key
	/// - `Integer`: Y coordinate of the public key
	/// - `Integer`: Private key
	///
	/// The TS3AudioBot stores two 1 bits in the first `BitString` and omits the
	/// public key.
	pub fn from_tomcrypt(data: &[u8]) -> Result<Self> {
		let blocks = ::simple_asn1::from_der(data)?;
		if blocks.len() != 1 {
			return Err(format_err!("More than one ASN.1 block").into());
		}
		if let ASN1Block::Sequence(_, blocks) = &blocks[0] {
			if let Some(ASN1Block::BitString(_, len, content)) = blocks.get(0) {
				if (*len != 1 && *len != 2) || content[0] & 0x80 == 0 {
					return Err(format_err!(
						"Does not contain a private key ({}, {:?})",
						len,
						content
					)
					.into());
				}
				if *len == 1 {
					if let Some(ASN1Block::Integer(_, i)) = blocks.get(4) {
						Self::from_short(i.to_bytes_be().1)
					} else {
						return Err(format_err!("Private key not found").into());
					}
				} else if let Some(ASN1Block::Integer(_, i)) = blocks.get(2) {
					Self::from_short(i.to_bytes_be().1)
				} else {
					return Err(format_err!("Private key not found").into());
				}
			} else {
				return Err(format_err!("Expected a bitstring").into());
			}
		} else {
			return Err(format_err!("Expected a sequence").into());
		}
	}

	/// Convert to base64 encoded private tomcrypt key.
	pub fn to_ts(&self) -> Result<String> {
		Ok(base64::encode(&self.to_tomcrypt()?))
	}

	/// Store as obfuscated TeamSpeak identity.
	pub fn to_ts_obfuscated(&self) -> Result<String> {
		let mut data = self.to_ts()?.into_bytes();
		// Xor first 100 bytes with a static value
		#[allow(clippy::needless_range_loop)]
		for i in 0..cmp::min(data.len(), 100) {
			data[i] ^= crate::IDENTITY_OBFUSCATION[i];
		}

		// Hash everything until the first 0 byte, starting after the first 20
		// bytes.
		let pos = data[20..]
			.iter()
			.position(|b| *b == b'\0')
			.unwrap_or(data.len() - 20);
		let hash = digest::digest(&digest::SHA1, &data[20..20 + pos]);
		let hash = hash.as_ref();
		// Xor first 20 bytes of data with the hash
		for i in 0..20 {
			data[i] ^= hash[i];
		}
		Ok(base64::encode(&data))
	}

	pub fn to_tomcrypt(&self) -> Result<Vec<u8>> {
		let pubkey = self.0.public_key();
		let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
		let mut ctx = BigNumContext::new()?;
		let pubkey_bin = pubkey.to_bytes(
			&group,
			ec::PointConversionForm::UNCOMPRESSED,
			&mut ctx,
		)?;
		let pub_len = (pubkey_bin.len() - 1) / 2;
		let pubkey_x =
			BigInt::from_bytes_be(Sign::Plus, &pubkey_bin[1..=pub_len]);
		let pubkey_y =
			BigInt::from_bytes_be(Sign::Plus, &pubkey_bin[1 + pub_len..]);

		let privkey =
			BigInt::from_bytes_be(Sign::Plus, &self.0.private_key().to_vec());

		Ok(::simple_asn1::to_der(&ASN1Block::Sequence(
			0,
			vec![
				ASN1Block::BitString(0, 1, vec![0x80]),
				ASN1Block::Integer(0, 32.into()),
				ASN1Block::Integer(0, pubkey_x),
				ASN1Block::Integer(0, pubkey_y),
				ASN1Block::Integer(0, privkey),
			],
		))?)
	}

	/// This has to be the private key, the other one has to be the public key.
	pub fn create_shared_secret(self, other: EccKeyPubP256) -> Result<Vec<u8>> {
		let privkey = PKey::from_ec_key(self.0)?;
		let pubkey = PKey::from_ec_key(other.0)?;
		let mut deriver = Deriver::new(&privkey)?;

		deriver.set_peer(&pubkey)?;

		let secret = deriver.derive_to_vec()?;
		Ok(secret)
	}

	pub fn sign(self, data: &[u8]) -> Result<Vec<u8>> {
		let pkey = PKey::from_ec_key(self.0)?;
		let mut signer = Signer::new(MessageDigest::sha256(), &pkey)?;
		signer.update(data)?;
		Ok(signer.sign_to_vec()?)
	}

	pub fn to_pub(&self) -> EccKeyPubP256 { self.into() }
}

impl<'a> Into<EccKeyPubP256> for &'a EccKeyPrivP256 {
	fn into(self) -> EccKeyPubP256 {
		EccKeyPubP256(
			EcKey::from_public_key(&self.0.group(), &self.0.public_key())
				.unwrap(),
		)
	}
}

impl EccKeyPubEd25519 {
	pub fn from_bytes(data: [u8; 32]) -> Self {
		EccKeyPubEd25519(CompressedEdwardsY(data))
	}

	pub fn from_base64(data: &str) -> Result<Self> {
		let decoded = base64::decode(data)?;
		if decoded.len() != 32 {
			return Err(format_err!("Wrong key length").into());
		}
		Ok(Self::from_bytes(*array_ref!(decoded, 0, 32)))
	}

	pub fn to_base64(&self) -> String {
		let EccKeyPubEd25519(CompressedEdwardsY(ref data)) = *self;
		base64::encode(data)
	}
}

impl EccKeyPrivEd25519 {
	/// This is not used to create TeamSpeak keys, as they are not canonical.
	pub fn create() -> Result<Self> {
		Ok(EccKeyPrivEd25519(Scalar::random(
			&mut ::rand::rngs::OsRng::new()?,
		)))
	}

	pub fn from_base64(data: &str) -> Result<Self> {
		let decoded = base64::decode(data)?;
		if decoded.len() != 32 {
			return Err(format_err!("Wrong key length").into());
		}
		Ok(Self::from_bytes(*array_ref!(decoded, 0, 32)))
	}

	pub fn from_bytes(data: [u8; 32]) -> Self {
		EccKeyPrivEd25519(Scalar::from_bytes_mod_order(data))
	}

	pub fn to_base64(&self) -> String { base64::encode(self.0.as_bytes()) }

	/// This has to be the private key, the other one has to be the public key.
	pub fn create_shared_secret(
		&self,
		pub_key: &EdwardsPoint,
	) -> Result<[u8; 32]>
	{
		let res = pub_key * self.0;
		Ok(res.compress().0)
	}

	pub fn to_pub(&self) -> EccKeyPubEd25519 { self.into() }
}

impl<'a> Into<EccKeyPubEd25519> for &'a EccKeyPrivEd25519 {
	fn into(self) -> EccKeyPubEd25519 {
		EccKeyPubEd25519(
			(&constants::ED25519_BASEPOINT_TABLE * &self.0).compress(),
		)
	}
}

/// This eax implementation uses AES-128 in counter mode for encryption and
/// AES-128 in CBC mode to generate the OMAC/CMAC/CBCMAC.
///
/// EAX is an AEAD (Authenticated Encryption with Associated Data) encryption
/// scheme.
pub struct Eax;

impl Eax {
	/// Encrypt and authenticate data.
	///
	/// # Arguments
	///
	/// - `header`: Associated data, which will also be authenticated.
	///
	/// # Return value
	///
	/// - tag/mac
	/// - Encrypted data
	pub fn encrypt(
		key: &[u8; 16],
		nonce: &[u8; 16],
		header: &[u8],
		data: &[u8],
	) -> Result<(Vec<u8>, Vec<u8>)>
	{
		// https://crypto.stackexchange.com/questions/26948/eax-cipher-mode-with-nonce-equal-header
		// has an explanation of eax.

		// l = block cipher size = 128 (for AES-128) = 16 byte
		// 1. n ← OMAC(0 || Nonce)
		// (the 0 means the number zero in l bits)
		let n = Self::cmac_with_iv(key, 0, nonce)?;

		// 2. h ← OMAC(1 || Nonce)
		let h = Self::cmac_with_iv(key, 1, header)?;

		// 3. enc ← CTR(M) using n as iv
		let enc = symm::encrypt(Cipher::aes_128_ctr(), key, Some(&n), data)?;

		// 4. c ← OMAC(2 || enc)
		let c = Self::cmac_with_iv(key, 2, &enc)?;

		// 5. tag ← n ^ h ^ c
		// (^ means xor)
		let mac: Vec<_> = n
			.iter()
			.zip(h.iter())
			.zip(c.iter())
			.map(|((n, h), c)| n ^ h ^ c)
			.collect();

		Ok((mac, enc))
	}

	pub fn decrypt(
		key: &[u8; 16],
		nonce: &[u8; 16],
		header: &[u8],
		data: &[u8],
		mac: &[u8],
	) -> Result<Vec<u8>>
	{
		let n = Self::cmac_with_iv(key, 0, nonce)?;

		// 2. h ← OMAC(1 || Nonce)
		let h = Self::cmac_with_iv(key, 1, header)?;

		// 4. c ← OMAC(2 || enc)
		let c = Self::cmac_with_iv(key, 2, data)?;

		let mac2: Vec<_> = n
			.iter()
			.zip(h.iter())
			.zip(c.iter())
			.map(|((n, h), c)| n ^ h ^ c)
			.take(mac.len())
			.collect();

		// Check mac using secure comparison
		if !::openssl::memcmp::eq(mac, &mac2) {
			return Err(Error::WrongMac);
		}

		// Decrypt
		let decrypt =
			symm::decrypt(Cipher::aes_128_ctr(), key, Some(&n), data)?;
		Ok(decrypt)
	}

	/// CMAC/OMAC1
	///
	/// To avoid constructing new buffers on the heap, an iv encoded into 16
	/// bytes is prepended inside this function.
	pub fn cmac_with_iv(
		key: &[u8; 16],
		iv: u8,
		data: &[u8],
	) -> Result<Vec<u8>>
	{
		let cipher = Cipher::aes_128_cbc();
		let key = PKey::cmac(&cipher, key)?;
		let mut signer = Signer::new_without_digest(&key)?;

		signer.update(&[0; 15])?;
		signer.update(&[iv])?;
		signer.update(data)?;

		let sign = signer.sign_to_vec()?;
		Ok(sign)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const TEST_PRIV_KEY: &str = "MG0DAgeAAgEgAiAIXJBlj1hQbaH0Eq0DuLlCmH8bl+veTA\
		O2+k9EQjEYSgIgNnImcmKo7ls5mExb6skfK2Tw+u54aeDr0OP1ITsC/50CIA8M5nmDB\
		nmDM/gZ//4AAAAAAAAAAAAAAAAAAAAZRzOI";

	#[test]
	fn parse_p256_priv_key() {
		EccKeyPrivP256::from_ts(TEST_PRIV_KEY).unwrap();
	}

	#[test]
	fn obfuscated_priv_key() {
		let key = EccKeyPrivP256::from_ts(TEST_PRIV_KEY).unwrap();
		let obf = key.to_ts_obfuscated().unwrap();
		let key2 = EccKeyPrivP256::from_ts_obfuscated(&obf).unwrap();
		assert_eq!(key.to_short(), key2.to_short());
	}

	#[test]
	fn obfuscated_identity() {
		let key = EccKeyPrivP256::from_ts(TEST_PRIV_KEY).unwrap();
		let uid = key.to_pub().get_uid().unwrap();

		let expected_uid = "lks7QL5OVMKo4pZ79cEOI5r5oEA=";
		assert_eq!(expected_uid, &uid);
	}

	#[test]
	fn test_p256_priv_key_short() {
		let key = EccKeyPrivP256::from_ts(TEST_PRIV_KEY).unwrap();
		let short = key.to_short();
		let key = EccKeyPrivP256::from_short(short.as_slice()).unwrap();
		let short2 = key.to_short();
		assert_eq!(short, short2);
	}

	#[test]
	fn parse_ed25519_pub_key() {
		EccKeyPubEd25519::from_base64(
			"zQ3irtRjRVCafjz9j2iz3HVVsp3M7HPNGHUPmTgSQIo=",
		)
		.unwrap();
	}
}
