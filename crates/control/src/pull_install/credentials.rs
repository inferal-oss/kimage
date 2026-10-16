//! Installation credentials are generated locally and reused by identity.
//! Only leaf keys enter serving Secrets; CA signing keys are never deployed.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client, ResourceExt};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use std::collections::BTreeMap;

pub type Material = BTreeMap<String, String>;

#[derive(Clone)]
pub struct Credentials {
    pub registry: Material,
    pub admission: Material,
    pub builder: Material,
}

fn random() -> Result<Vec<u8>> {
    let mut bytes = vec![0; 32];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("CredentialEntropyUnavailable"))?;
    Ok(bytes)
}
fn encoded(value: impl AsRef<[u8]>) -> String {
    STANDARD.encode(value)
}
fn parameters(names: Vec<String>) -> Result<CertificateParams> {
    let mut params = CertificateParams::new(names)?;
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(5);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(3650);
    Ok(params)
}
fn authority(name: &str) -> Result<(String, Issuer<'static, KeyPair>)> {
    let mut params = parameters(Vec::new())?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let mut distinguished = DistinguishedName::new();
    distinguished.push(DnType::CommonName, format!("{name} CA"));
    params.distinguished_name = distinguished;
    let key = KeyPair::generate()?;
    let certificate = params.self_signed(&key)?.pem();
    Ok((certificate, Issuer::new(params, key)))
}
fn leaf(
    names: Vec<String>,
    client: bool,
    issuer: &Issuer<'_, KeyPair>,
) -> Result<(String, String)> {
    let mut params = parameters(names)?;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![if client {
        ExtendedKeyUsagePurpose::ClientAuth
    } else {
        ExtendedKeyUsagePurpose::ServerAuth
    }];
    let key = KeyPair::generate()?;
    Ok((params.signed_by(&key, issuer)?.pem(), key.serialize_pem()))
}
fn tls(names: Vec<String>) -> Result<Material> {
    let (ca, issuer) = authority(&names[0])?;
    let (cert, key) = leaf(names, false, &issuer)?;
    Ok(BTreeMap::from([
        ("ca.crt".into(), encoded(ca)),
        ("tls.crt".into(), encoded(cert)),
        ("tls.key".into(), encoded(key)),
    ]))
}
impl Credentials {
    pub fn generate(namespace: &str, installation: &str) -> Result<Self> {
        let dns = |role: &str| format!("{installation}-{role}.{namespace}.svc");
        let mut registry = tls(vec![dns("registry")])?;
        registry.insert("reader-password".into(), encoded(hex::encode(random()?)));
        registry.insert("publisher-password".into(), encoded(hex::encode(random()?)));
        let mut admission = tls(vec![dns("admission")])?;
        admission.insert("resolution-key".into(), encoded(random()?));
        let (ca, issuer) = authority(&dns("builder"))?;
        let (server, server_key) = leaf(
            vec![dns("builder"), dns("builder-amd64"), dns("builder-arm64")],
            false,
            &issuer,
        )?;
        let (client, client_key) = leaf(Vec::new(), true, &issuer)?;
        let builder = BTreeMap::from([
            ("ca.crt".into(), encoded(ca)),
            ("server.crt".into(), encoded(server)),
            ("server.key".into(), encoded(server_key)),
            ("client.crt".into(), encoded(client)),
            ("client.key".into(), encoded(client_key)),
        ]);
        Ok(Self {
            registry,
            admission,
            builder,
        })
    }

    pub async fn resolve(client: Client, namespace: &str, installation: &str) -> Result<Self> {
        let mut credentials = Self::generate(namespace, installation)?;
        let secrets: Api<Secret> = Api::namespaced(client, namespace);
        for (suffix, material) in [
            ("registry-credentials", &mut credentials.registry),
            ("admission-credentials", &mut credentials.admission),
            ("builder-credentials", &mut credentials.builder),
        ] {
            if let Some(secret) = secrets.get_opt(&format!("{installation}-{suffix}")).await? {
                ensure!(
                    secret
                        .labels()
                        .get(crate::install::LABEL)
                        .is_some_and(|label| label == installation),
                    "CredentialOwnershipConflict"
                );
                let data = secret.data.context("MissingInstallationCredentials")?;
                ensure!(
                    data.keys().eq(material.keys())
                        && data
                            .values()
                            .all(|value| !value.0.is_empty() && value.0.len() <= 65536),
                    "InvalidInstallationCredentials"
                );
                *material = data
                    .into_iter()
                    .map(|(key, value)| (key, encoded(value.0)))
                    .collect();
            }
        }
        credentials.validate()?;
        Ok(credentials)
    }

    pub fn subset(material: &Material, keys: &[&str]) -> Result<Material> {
        keys.iter()
            .map(|key| {
                Ok((
                    (*key).to_owned(),
                    material
                        .get(*key)
                        .context("MissingInstallationCredential")?
                        .clone(),
                ))
            })
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        for (material, prefixes) in [
            (&self.registry, vec!["tls"]),
            (&self.admission, vec!["tls"]),
            (&self.builder, vec!["server", "client"]),
        ] {
            reqwest::Certificate::from_pem(
                &STANDARD.decode(material.get("ca.crt").context("MissingCredentialCa")?)?,
            )?;
            for prefix in prefixes {
                let mut identity = STANDARD.decode(
                    material
                        .get(&format!("{prefix}.crt"))
                        .context("MissingCredentialCertificate")?,
                )?;
                identity.extend(
                    STANDARD.decode(
                        material
                            .get(&format!("{prefix}.key"))
                            .context("MissingCredentialKey")?,
                    )?,
                );
                reqwest::Identity::from_pem(&identity)
                    .context("InvalidInstallationCertificateIdentity")?;
            }
        }
        ensure!(
            self.registry.get("reader-password") != self.registry.get("publisher-password"),
            "IndependentRegistryCredentialsRequired"
        );
        for key in ["reader-password", "publisher-password"] {
            ensure!(
                STANDARD
                    .decode(self.registry.get(key).context("MissingRegistryPassword")?)?
                    .len()
                    >= 32,
                "InvalidRegistryPassword"
            );
        }
        ensure!(
            STANDARD
                .decode(
                    self.admission
                        .get("resolution-key")
                        .context("MissingResolutionKey")?
                )?
                .len()
                >= 32,
            "InvalidResolutionKey"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_identities_are_independent_and_have_no_ca_signing_keys() {
        let credentials = Credentials::generate("kimage-system", "kimage").unwrap();
        credentials.validate().unwrap();
        assert_ne!(
            credentials.registry["ca.crt"],
            credentials.admission["ca.crt"]
        );
        assert_ne!(
            credentials.registry["ca.crt"],
            credentials.builder["ca.crt"]
        );
        assert_ne!(
            credentials.builder["server.key"],
            credentials.builder["client.key"]
        );
        assert!(
            credentials
                .registry
                .keys()
                .chain(credentials.admission.keys())
                .chain(credentials.builder.keys())
                .all(|key| !key.starts_with("ca.key"))
        );
        let mut invalid = credentials.clone();
        invalid.registry.insert(
            "publisher-password".into(),
            invalid.registry["reader-password"].clone(),
        );
        assert!(invalid.validate().is_err());
    }
}
