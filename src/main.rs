use kube::CustomResourceExt;

#[tokio::main]
async fn main() {
    println!(
        "{}",
        serde_yaml::to_string(&operator::types::v1alpha1::tenant::Tenant::crd()).unwrap()
    )
}
