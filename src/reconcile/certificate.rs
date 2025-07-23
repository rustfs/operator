// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![allow(dead_code)]

use crate::context::Context;
use crate::error::Error;
use crate::types::v1alpha1::tenant::Tenant;

use crate::utils::tls;
use k8s_openapi::api::core::v1 as corev1;

pub async fn check_certificate_status(tenant: &Tenant, ctx: &Context) -> Result<(), Error> {
    let secret = ctx
        .get::<corev1::Secret>(&tenant.secret_name(), &tenant.namespace()?)
        .await?;

    Ok(())
}

// check the secret need renew or not.
fn renew(secret: &corev1::Secret) -> Result<bool, Error> {
    let Some(ref data) = secret.data else {
        return Err(Error::StrError("empty data for minio secret".into()));
    };

    let (pub_key, pri_key) = match secret.type_.as_deref() {
        Some("kubernetes.io/tls")
        | Some("cert-manager.io/v1alpha2")
        | Some("cert-manager.io/v1") => ("tls.crt", "tls.key"),
        _ => ("public.crt", "private.key"),
    };

    let cert_pub_key = data
        .get(pub_key)
        .ok_or(Error::StrError("miss public key".into()))?;

    let cert_pri_key = data
        .get(pri_key)
        .ok_or(Error::StrError("miss private key".into()))?;

    tls::x509_key_pair(&cert_pub_key.0[..], &cert_pri_key.0[..]);

    Ok(false)
}
