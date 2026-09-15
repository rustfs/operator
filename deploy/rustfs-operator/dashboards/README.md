# Bundled RustFS Grafana dashboards

The JSON files in this directory are vendored from
[`rustfs/rustfs`](https://github.com/rustfs/rustfs/tree/main/.docker/observability/grafana/dashboards)
at revision `6ef0740afeb5ae3bc890774b8e83d7e21c94a7e3`.

Keep the filenames stable because the Helm template uses them for ConfigMap names
and data keys. When updating the dashboards, copy all JSON files from the same
RustFS revision and run the chart tests before changing the source revision above.
