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

use crate::error::Error;
use crate::types::v1alpha1::tenant::Tenant;
use k8s_openapi::NamespaceResourceScope;
use kube::api::{DeleteParams, ListParams, ObjectList, Patch, PatchParams, PostParams};
use kube::runtime::events::{Event, EventType, Recorder, Reporter};
use kube::{Resource, ResourceExt, api::Api};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use tracing::info;

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
            .await?;

        Ok(())
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
            let status_body = serde_json::to_vec(&status)?;

            api.replace_status(name, &PostParams::default(), status_body)
                .await
                .map_err(Error::from)
        };

        match update_func(resource).await {
            Ok(t) => return Ok(t),
            Err(e) if !e.is_conflict() => return Err(e),
            _ => {}
        }

        info!("status update failed due to conflict, retrieve the latest resource and retry.");

        let new_one = api.get(name).await.map_err(Error::from)?;
        update_func(&new_one).await
    }

    pub async fn delete<T>(&self, name: &str, namespace: &str) -> Result<(), Error>
    where
        T: Resource<Scope = NamespaceResourceScope> + Clone + DeserializeOwned + Debug,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.delete(name, &DeleteParams::default()).await?;
        Ok(())
    }

    pub async fn get<T>(&self, name: &str, namespace: &str) -> Result<T, Error>
    where
        T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        Ok(api.get(name).await?)
    }

    pub async fn create<T>(&self, resource: &T, namespace: &str) -> Result<T, Error>
    where
        T: Clone + Serialize + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        Ok(api.create(&PostParams::default(), resource).await?)
    }

    pub async fn list<T>(&self, namespace: &str) -> Result<ObjectList<T>, Error>
    where
        T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        Ok(api.list(&ListParams::default()).await?)
    }

    pub async fn apply<T>(&self, resource: T, namespace: &str) -> Result<T, Error>
    where
        T: Clone + Serialize + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        Ok(api
            .patch(
                &resource.name_any(),
                &PatchParams::apply("rustfs-operator"),
                &Patch::Apply(resource),
            )
            .await?)
    }
}
