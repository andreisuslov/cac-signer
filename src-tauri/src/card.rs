//! The CAC through macOS CryptoTokenKit: list signing certificates, sign, verify.
//! The private key never leaves the card, and macOS asks for the PIN itself.

use core_foundation::base::TCFType;
use security_framework::base::Error;
use security_framework::certificate::SecCertificate;
use security_framework::cms::{CMSDecoder, CMSEncoder, CMS_DIGEST_ALGORITHM_SHA256};
use security_framework::identity::SecIdentity;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
use security_framework::key::Algorithm;
use security_framework::policy::SecPolicy;
use security_framework_sys::cms::{
    CMSCertificateChainMode, CMSDecoderCopySignerCert, CMSSignerStatus,
};
use serde::Serialize;
use x509_parser::prelude::*;

/// id-PIV-cardAuth. That key works without a PIN, so it is never offered for signing.
const PIV_CARD_AUTH: &str = "2.16.840.1.101.3.6.8";
const EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_USER_CANCELED: i32 = -128;

#[derive(Serialize, Clone)]
pub struct Identity {
    /// SHA-1 of the certificate, hex.
    pub id: String,
    pub kind: &'static str,
    pub name: String,
    pub issuer: String,
    pub expires: String,
    pub expired: bool,
}

pub struct Verified {
    pub signer: String,
    /// "valid", "untrusted" (signature matches, certificate chain not trusted here) or "invalid".
    pub status: &'static str,
    pub detail: String,
}

fn token_identities() -> Result<Vec<SecIdentity>, String> {
    let found = ItemSearchOptions::new()
        .class(ItemClass::identity())
        .access_group_token()
        .load_refs(true)
        .limit(Limit::All)
        .search();
    match found {
        Ok(items) => Ok(items
            .into_iter()
            .filter_map(|item| match item {
                SearchResult::Ref(Reference::Identity(identity)) => Some(identity),
                _ => None,
            })
            .collect()),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(Vec::new()),
        Err(e) => Err(format!("Could not read the smart card: {e}")),
    }
}

fn describe(identity: &SecIdentity) -> Option<Identity> {
    let der = identity.certificate().ok()?.to_der();
    let (_, cert) = parse_x509_certificate(&der).ok()?;
    let usage = cert.key_usage().ok()??;
    let card_auth = cert.extended_key_usage().ok().flatten().is_some_and(|eku| {
        eku.value.other.iter().any(|oid| oid.to_id_string() == PIV_CARD_AUTH)
    });
    let kind = if card_auth {
        return None;
    } else if usage.value.non_repudiation() {
        "Digital Signature"
    } else if usage.value.digital_signature() {
        "PIV Authentication"
    } else {
        return None; // encryption key
    };
    let cn = |name: &X509Name| {
        name.iter_common_name()
            .next()
            .and_then(|attr| attr.as_str().ok())
            .unwrap_or("?")
            .to_string()
    };
    let not_after = cert.validity().not_after.timestamp();
    Some(Identity {
        id: sha1_smol::Sha1::from(&der).digest().to_string(),
        kind,
        name: cn(cert.subject()),
        issuer: cn(cert.issuer()),
        expires: chrono::DateTime::from_timestamp(not_after, 0)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        expired: not_after < chrono::Utc::now().timestamp(),
    })
}

/// Signing certificates on inserted cards, the Digital Signature certificate first.
pub fn list_identities() -> Result<Vec<Identity>, String> {
    let mut list: Vec<Identity> = token_identities()?.iter().filter_map(describe).collect();
    list.sort_by_key(|i| (i.expired, i.kind != "Digital Signature"));
    Ok(list)
}

pub fn find(id: &str) -> Result<(SecIdentity, Identity), String> {
    token_identities()?
        .into_iter()
        .find_map(|identity| {
            let info = describe(&identity).filter(|info| info.id == id)?;
            Some((identity, info))
        })
        .ok_or_else(|| "That certificate is not available. Is the card still inserted?".into())
}

/// Brings up the PIN prompt now, if the card wants one, by signing a throwaway value.
pub fn unlock(identity: &SecIdentity) -> Result<(), String> {
    let cert = identity.certificate().map_err(|e| format!("Signing failed: {e}"))?.to_der();
    let ec = parse_x509_certificate(&cert)
        .is_ok_and(|(_, c)| c.public_key().algorithm.algorithm.to_id_string() == EC_PUBLIC_KEY);
    let algorithm = if ec {
        Algorithm::ECDSASignatureMessageX962SHA256
    } else {
        Algorithm::RSASignatureMessagePKCS1v15SHA256
    };
    let key = identity.private_key().map_err(|e| format!("Signing failed: {e}"))?;
    key.create_signature(algorithm, b"cac-signer: unlock")
        .map(drop)
        .map_err(|e| match e.code() {
            -128 | -2 => "Signing was cancelled.".to_string(), // errSecUserCanceled, LAErrorUserCancel
            _ => format!("Signing failed: {}", e.description()),
        })
}

/// Detached CMS signature over `data` (SHA-256, certificate chain up to the root).
pub fn sign_detached(identity: &SecIdentity, data: &[u8]) -> Result<Vec<u8>, String> {
    let fail = |e: Error| {
        if e.code() == ERR_SEC_USER_CANCELED {
            "Signing was cancelled.".to_string()
        } else {
            format!("Signing failed: {e}")
        }
    };
    let encoder = CMSEncoder::create().map_err(fail)?;
    encoder.set_signer_algorithm(CMS_DIGEST_ALGORITHM_SHA256).map_err(fail)?;
    encoder.add_signers(std::slice::from_ref(identity)).map_err(fail)?;
    encoder.set_has_detached_content(true).map_err(fail)?;
    encoder
        .set_certificate_chain_mode(CMSCertificateChainMode::kCMSCertificateChainWithRoot)
        .map_err(fail)?;
    encoder.update_content(data).map_err(fail)?;
    encoder.get_encoded_content().map_err(fail)
}

/// Checks a detached CMS signature over `content` against the certificates this Mac trusts.
pub fn verify_detached(cms: &[u8], content: &[u8]) -> Verified {
    let check = || -> Result<Verified, Error> {
        let decoder = CMSDecoder::create()?;
        decoder.update_message(cms)?;
        decoder.set_detached_content(content)?;
        decoder.finalize_message()?;
        let status = decoder.get_signer_status(0, &[SecPolicy::create_x509()])?;
        let mut cert = std::ptr::null_mut();
        let copied = unsafe { CMSDecoderCopySignerCert(decoder.as_concrete_TypeRef(), 0, &mut cert) };
        let signer = if copied == 0 && !cert.is_null() {
            unsafe { SecCertificate::wrap_under_create_rule(cert) }.subject_summary()
        } else {
            "unknown signer".into()
        };
        let (status, detail) = match status.signer_status {
            CMSSignerStatus::kCMSSignerValid => ("valid", String::new()),
            CMSSignerStatus::kCMSSignerInvalidCert => (
                "untrusted",
                status.cert_verify_result.err().map(|e| e.to_string()).unwrap_or_default(),
            ),
            _ => ("invalid", "The signature does not match the document.".into()),
        };
        Ok(Verified { signer, status, detail })
    };
    check().unwrap_or_else(|e| Verified {
        signer: "unknown signer".into(),
        status: "invalid",
        detail: format!("Could not read the signature: {e}"),
    })
}
