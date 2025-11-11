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

use crate::types;
use crate::types::v1alpha1::tenant::Tenant;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::core::v1::Secret;
use kube::api::{DeleteParams, ListParams, ObjectList, Patch, PatchParams, PostParams};
use kube::runtime::events::{Event, EventType, Recorder, Reporter};
use kube::{Resource, ResourceExt, api::Api};
use serde::Serialize;
use serde::de::DeserializeOwned;
use snafu::Snafu;
use snafu::futures::TryFutureExt;
use std::collections::BTreeMap;
use std::fmt::Debug;
use tracing::info;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Kubernetes API error: {}", source))]
    Kube { source: kube::Error },

    #[snafu(display("record event error: {}", source))]
    Record { source: kube::Error },

    #[snafu(transparent)]
    Types { source: types::error::Error },

    #[snafu(display("empty tenant credentials"))]
    EmptyRootCredentials,
}

pub struct Context {
    pub(crate) client: kube::Client,
    pub(crate) recorder: Recorder,
}

impl Context {
    pub fn new(client: kube::Client) -> Self {
        let reporter = Reporter {
            controller: "rustfs-operator".into(),
            instance: std::env::var("HOSTNAME").ok(),
        };

        let recorder = Recorder::new(client.clone(), reporter);
        Self { client, recorder }
    }

    /// send event
    #[inline]
    pub async fn record(
        &self,
        resource: &Tenant,
        event_type: EventType,
        reason: &str,
        message: &str,
    ) -> Result<(), Error> {
        self.recorder
            .publish(
                &Event {
                    type_: event_type,
                    reason: reason.to_owned(),
                    note: Some(message.into()),
                    action: "Reconcile".into(),
                    secondary: None,
                },
                &resource.object_ref(&()),
            )
            .context(RecordSnafu)
            .await
    }

    pub async fn update_status<S>(
        &self,
        resource: &Tenant,
        current_status: S,
        replica: i32,
    ) -> Result<Tenant, Error>
    where
        S: ToString,
    {
        let api: Api<Tenant> = Api::namespaced(self.client.clone(), &resource.namespace()?);
        let name = &resource.name();

        let update_func = async |tenant: &Tenant| {
            let mut status = tenant.status.clone().unwrap_or_default();
            status.available_replicas = replica;
            status.current_state = current_status.to_string();
            let status_body = serde_json::to_vec(&status).unwrap();

            api.replace_status(name, &PostParams::default(), status_body)
                .context(KubeSnafu)
                .await
        };

        match update_func(resource).await {
            Ok(t) => return Ok(t),
            _ => {}
        }

        info!("status update failed due to conflict, retrieve the latest resource and retry.");

        let new_one = api.get(name).context(KubeSnafu).await?;
        update_func(&new_one).await
    }

    pub async fn delete<T>(&self, name: &str, namespace: &str) -> Result<(), Error>
    where
        T: Resource<Scope = NamespaceResourceScope> + Clone + DeserializeOwned + Debug,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.delete(name, &DeleteParams::default())
            .context(KubeSnafu)
            .await?;
        Ok(())
    }

    pub async fn get<T>(&self, name: &str, namespace: &str) -> Result<T, Error>
    where
        T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.get(name).context(KubeSnafu).await
    }

    pub async fn create<T>(&self, resource: &T, namespace: &str) -> Result<T, Error>
    where
        T: Clone + Serialize + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.create(&PostParams::default(), resource)
            .context(KubeSnafu)
            .await
    }

    pub async fn list<T>(&self, namespace: &str) -> Result<ObjectList<T>, Error>
    where
        T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.list(&ListParams::default()).context(KubeSnafu).await
    }

    pub async fn apply<T>(&self, resource: &T, namespace: &str) -> Result<T, Error>
    where
        T: Clone + Serialize + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.patch(
            &resource.name_any(),
            &PatchParams::apply("rustfs-operator"),
            &Patch::Apply(resource),
        )
        .context(KubeSnafu)
        .await
    }

    pub async fn get_tenant_credentials(
        &self,
        tenant: &Tenant,
    ) -> Result<BTreeMap<String, String>, Error> {
        let config: std::collections::BTreeMap<_, _> = tenant
            .spec
            .env
            .iter()
            .filter_map(|item| item.value.as_ref().map(|v| (item.name.clone(), v.clone())))
            .collect();

        if let Some(ref cfg) = tenant.spec.configuration
            && !cfg.name.is_empty()
        {
            // todo: add env from Secret
            let _secret: Secret = self.get(&cfg.name, &tenant.namespace()?).await?;
        }

        // if no ak/sk or ak/sk is empty
        if config.get("accesskey").is_none_or(|x| x.is_empty())
            || config.get("secretkey").is_none_or(|x| x.is_empty())
        {
            return EmptyRootCredentialsSnafu.fail();
        }

        Ok(config)
    }
}
