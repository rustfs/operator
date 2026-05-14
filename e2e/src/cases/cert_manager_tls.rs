// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{CaseSpec, Suite};

pub fn cases() -> Vec<CaseSpec> {
    vec![
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_managed_certificate_reaches_tls_ready_and_https_wiring",
            "Apply a cert-manager managed Certificate Tenant in an isolated namespace/storage layout and verify TLSReady, Certificate Ready, HTTPS services, probes, RUSTFS_VOLUMES, and TLS file mounts.",
            "cert-manager/managed-certificate",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_external_secret_reaches_tls_ready_and_rolls_on_secret_hash",
            "Use an isolated namespace/storage layout with a pre-created kubernetes.io/tls Secret plus external CA Secret and verify TLS hash changes trigger rollout wiring.",
            "cert-manager/external-secret",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_rejects_secret_missing_tls_crt",
            "Verify an API-admissible configured Opaque Secret without tls.crt reports TLSReady=False with CertificateSecretMissingKey.",
            "cert-manager/negative-secret",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_rejects_secret_missing_tls_key",
            "Verify an API-admissible configured Opaque Secret without tls.key reports TLSReady=False with CertificateSecretMissingKey.",
            "cert-manager/negative-secret",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_rejects_secret_missing_ca_for_internode_https",
            "Verify enableInternodeHttps without trusted CA material reports TLSReady=False with CaBundleMissing.",
            "cert-manager/negative-ca-trust",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_rejects_missing_issuer_for_managed_certificate",
            "Verify manageCertificate=true with a missing Issuer reports TLSReady=False with CertManagerIssuerNotFound.",
            "cert-manager/negative-issuer",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_reports_pending_certificate_not_ready",
            "Verify a cert-manager Certificate that remains Pending/NotReady reports TLSReady=False with CertManagerCertificateNotReady.",
            "cert-manager/certificate-pending",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_rejects_hot_reload",
            "Verify rotationStrategy=HotReload remains blocked until RustFS supports clean-directory reload.",
            "cert-manager/negative-hot-reload",
            "cert-manager-tls",
        ),
        CaseSpec::new(
            Suite::CertManagerTls,
            "cert_manager_artifacts_do_not_expose_secret_material",
            "Ensure generated manifests, command displays, status assertions, and artifact collection paths do not print PEM or Secret payloads.",
            "cert-manager/security",
            "cert-manager-tls",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn cert_manager_case_inventory_matches_expected_order() {
        let names = cases()
            .into_iter()
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert_eq!(names.len(), 9);
        assert_eq!(
            names.first().copied(),
            Some("cert_manager_managed_certificate_reaches_tls_ready_and_https_wiring")
        );
        assert_eq!(
            names.last().copied(),
            Some("cert_manager_artifacts_do_not_expose_secret_material")
        );
    }
}
