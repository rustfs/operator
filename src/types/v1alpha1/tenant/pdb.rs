use super::Tenant;
use crate::types;
use crate::types::v1alpha1::pool::Pool;
use k8s_openapi::api::policy::v1 as policyv1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

impl Tenant {
    pub fn new_pdb(
        &self,
        pool: &Pool,
    ) -> Result<policyv1::PodDisruptionBudget, types::error::Error> {
        let labels = self.pool_labels(pool);
        let selector_labels = self.pool_selector_labels(pool);
        let name = format!("{}-{}", self.name(), pool.name);

        // IntOrString::Int(1) means max 1 pod can be unavailable at a time.
        // This is safe for most distributed storage upgrades/maintenance.
        let max_unavailable = Some(IntOrString::Int(1));

        Ok(policyv1::PodDisruptionBudget {
            metadata: metav1::ObjectMeta {
                name: Some(name),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(policyv1::PodDisruptionBudgetSpec {
                max_unavailable,
                selector: Some(metav1::LabelSelector {
                    match_labels: Some(selector_labels),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}
