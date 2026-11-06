// Copyright 2026 RustFS Team
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

use super::apply_statefulset;
use crate::context::{Context, Error};
use crate::types::v1alpha1::tenant::Tenant;
use futures::{FutureExt, StreamExt, TryStreamExt};
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{Container, Namespace, Pod, PodSpec};
use kube::api::{
    DeleteParams, ListParams, Patch, PatchParams, PostParams, WatchEvent, WatchParams,
};
use kube::config::KubeConfigOptions;
use kube::{Api, Client, Config, ResourceExt};
use serde_json::json;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

const SERVER: &str = r#"
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        time.sleep(float(os.environ["PROBE_DELAY"]))
        self.send_response(200)
        self.end_headers()

    def log_message(self, *args):
        pass

ThreadingHTTPServer(("0.0.0.0", 9000), Handler).serve_forever()
"#;

fn rollout_template(tenant: &Tenant, policy: &str, delay: &str, timeout: i32) -> StatefulSet {
    let mut desired = tenant
        .new_statefulset(&tenant.spec.pools[0])
        .expect("StatefulSet should render");
    let spec = desired.spec.as_mut().unwrap();
    spec.replicas = Some(2);
    spec.pod_management_policy = Some(policy.to_string());
    spec.volume_claim_templates = None;
    spec.template.spec = Some(
        serde_json::from_value::<PodSpec>(json!({
            "terminationGracePeriodSeconds": 1,
            "containers": [{
                "name": "rustfs",
                "image": "python:3.12-slim",
                "imagePullPolicy": "Never",
                "command": ["python", "-u", "-c", SERVER],
                "env": [{"name": "PROBE_DELAY", "value": delay}],
                "readinessProbe": {
                    "httpGet": {"path": "/health/ready", "port": 9000, "scheme": "HTTP"},
                    "initialDelaySeconds": 0,
                    "timeoutSeconds": timeout,
                    "periodSeconds": 1,
                    "failureThreshold": 1,
                    "successThreshold": 1
                }
            }]
        }))
        .expect("test Pod spec should deserialize"),
    );
    desired
}

fn probe_pair(container: &Container) -> (&str, i32) {
    let delay = container
        .env
        .as_ref()
        .unwrap()
        .iter()
        .find(|env| env.name == "PROBE_DELAY")
        .and_then(|env| env.value.as_deref())
        .expect("test server should have a probe delay");
    let timeout = container
        .readiness_probe
        .as_ref()
        .unwrap()
        .timeout_seconds
        .unwrap();
    (delay, timeout)
}

fn assert_complete_template(container: &Container) {
    let pair = probe_pair(container);
    assert!(
        matches!(pair, ("2", 5) | ("0", 1)),
        "readiness timing and server configuration must change together, observed {pair:?}"
    );
}

fn pod_ready(pod: &Pod) -> bool {
    pod.metadata.deletion_timestamp.is_none()
        && pod
            .status
            .as_ref()
            .and_then(|status| status.conditions.as_ref())
            .is_some_and(|conditions| {
                conditions
                    .iter()
                    .any(|c| c.type_ == "Ready" && c.status == "True")
            })
}

async fn wait_for_rollout(statefulsets: &Api<StatefulSet>, pods: &Api<Pod>, name: &str) {
    tokio::time::timeout(Duration::from_secs(120), async {
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        loop {
            interval.tick().await;
            let statefulset = statefulsets
                .get(name)
                .await
                .expect("StatefulSet should read");
            let Some(status) = statefulset.status.as_ref() else {
                continue;
            };
            if status.observed_generation != statefulset.metadata.generation
                || status.updated_replicas != Some(2)
                || status.ready_replicas != Some(2)
                || status.current_revision.is_none()
                || status.current_revision != status.update_revision
            {
                continue;
            }
            let observed = pods
                .list(&ListParams::default())
                .await
                .expect("Pods should list");
            let matching: Vec<_> = observed
                .items
                .iter()
                .filter(|pod| pod.name_any().starts_with(&format!("{name}-")))
                .collect();
            if matching.len() == 2
                && matching.iter().all(|pod| {
                    pod_ready(pod)
                        && pod.labels().get("controller-revision-hash")
                            == status.update_revision.as_ref()
                })
            {
                return;
            }
        }
    })
    .await
    .expect("both Pods should become Ready at the current StatefulSet revision");
}

async fn run_rollout(client: Client, namespace: &str, policy: &str) {
    let tenants: Api<Tenant> = Api::namespaced(client.clone(), namespace);
    let statefulsets: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let ctx = Context::new(client);
    let mut tenant = crate::tests::create_test_tenant(None, None);
    tenant.metadata.name = Some(format!("readiness-{}", policy.to_lowercase()));
    tenant.metadata.namespace = Some(namespace.to_string());
    tenant.metadata.uid = None;
    let tenant = tenants
        .create(&PostParams::default(), &tenant)
        .await
        .expect("test Tenant should create");
    let initial = rollout_template(&tenant, policy, "2", 5);
    let name = initial.name_any();
    ctx.apply(&initial, namespace)
        .await
        .expect("initial StatefulSet should create");
    wait_for_rollout(&statefulsets, &pods, &name).await;

    // Shared ownership forces the production helper to exercise conflict handling.
    let existing = statefulsets
        .patch(
            &name,
            &PatchParams::apply("external-readiness"),
            &Patch::Apply(json!({
                "apiVersion": "apps/v1", "kind": "StatefulSet",
                "metadata": {"name": name, "namespace": namespace},
                "spec": {"template": {"spec": {"containers": [{
                    "name": "rustfs", "readinessProbe": {"timeoutSeconds": 5}
                }]}}}
            })),
        )
        .await
        .expect("external manager should share the current readiness timeout");
    let mut templates = statefulsets
        .watch(
            &WatchParams::default()
                .fields(&format!("metadata.name={name}"))
                .timeout(150),
            existing.metadata.resource_version.as_deref().unwrap(),
        )
        .await
        .expect("StatefulSet watch should start")
        .boxed();
    let pod_list = pods
        .list(&ListParams::default())
        .await
        .expect("initial Pods should list");
    let mut pod_events = pods
        .watch(
            &WatchParams::default().timeout(150),
            pod_list.metadata.resource_version.as_deref().unwrap(),
        )
        .await
        .expect("Pod watch should start")
        .boxed();
    let desired = rollout_template(&tenant, policy, "0", 1);
    let updated = tokio::time::timeout(Duration::from_secs(15), async {
        let mut current = existing;
        loop {
            match apply_statefulset(&ctx, &tenant, namespace, &current, desired.clone()).await {
                Ok(updated) => return updated,
                Err(Error::Kube {
                    source: kube::Error::Api(response),
                }) if response.code == 409
                    && response.message.contains("the object has been modified") =>
                {
                    current = statefulsets
                        .get(&name)
                        .await
                        .expect("StatefulSet should refresh");
                }
                Err(error) => panic!("complete template update should succeed: {error}"),
            }
        }
    })
    .await
    .expect("template update should finish within its deadline");

    let check_templates = async {
        while let Some(event) = templates
            .try_next()
            .await
            .expect("StatefulSet watch should read")
        {
            match event {
                WatchEvent::Added(observed) | WatchEvent::Modified(observed) => {
                    let container = &observed
                        .spec
                        .as_ref()
                        .unwrap()
                        .template
                        .spec
                        .as_ref()
                        .unwrap()
                        .containers[0];
                    assert_complete_template(container);
                    if observed.metadata.resource_version == updated.metadata.resource_version {
                        assert_eq!(probe_pair(container), ("0", 1));
                        return;
                    }
                }
                WatchEvent::Error(error) => panic!("StatefulSet watch failed: {error:?}"),
                _ => {}
            }
        }
        panic!("StatefulSet watch ended before the submitted template");
    };
    let check_pods = async {
        let mut ready = HashSet::new();
        while let Some(event) = pod_events.try_next().await.expect("Pod watch should read") {
            match event {
                WatchEvent::Added(pod) | WatchEvent::Modified(pod)
                    if pod.name_any().starts_with(&format!("{name}-")) =>
                {
                    let container = &pod.spec.as_ref().unwrap().containers[0];
                    assert_complete_template(container);
                    if probe_pair(container) == ("0", 1) && pod_ready(&pod) {
                        ready.insert(pod.name_any());
                    } else {
                        ready.remove(&pod.name_any());
                    }
                    if ready.len() == 2 {
                        return;
                    }
                }
                WatchEvent::Deleted(pod) => {
                    ready.remove(&pod.name_any());
                }
                WatchEvent::Error(error) => panic!("Pod watch failed: {error:?}"),
                _ => {}
            }
        }
        panic!("Pod watch ended before both updated Pods became Ready");
    };
    tokio::time::timeout(Duration::from_secs(120), async {
        tokio::join!(check_templates, check_pods);
    })
    .await
    .expect("complete template rollout should finish without deleting Pods manually");
    wait_for_rollout(&statefulsets, &pods, &name).await;
}

#[tokio::test]
#[ignore = "requires RUSTFS_READINESS_TEST_CONTEXT, the Tenant CRD and python:3.12-slim on every node"]
async fn readiness_rollout_live_updates_complete_templates() {
    crate::install_rustls_crypto_provider();
    let context = std::env::var("RUSTFS_READINESS_TEST_CONTEXT")
        .expect("set RUSTFS_READINESS_TEST_CONTEXT to a disposable Kubernetes context");
    let mut config = Config::from_kubeconfig(&KubeConfigOptions {
        context: Some(context),
        ..Default::default()
    })
    .await
    .expect("dedicated readiness test context should load");
    config.proxy_url = None;
    let client = Client::try_from(config).expect("Kubernetes client should initialize");
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let namespace = namespaces
        .create(
            &PostParams::default(),
            &Namespace {
                metadata: kube::core::ObjectMeta {
                    generate_name: Some("readiness-rollout-".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .expect("isolated test namespace should create")
        .name_any();
    let result = AssertUnwindSafe(async {
        for policy in ["Parallel", "OrderedReady"] {
            run_rollout(client.clone(), &namespace, policy).await;
        }
    })
    .catch_unwind()
    .await;
    let cleanup = namespaces
        .delete(&namespace, &DeleteParams::default())
        .await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    cleanup.expect("isolated test namespace should delete");
}
