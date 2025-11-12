use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Namespace;
use kube::{
    api::{Api, DynamicObject, ListParams},
    discovery::{self, Scope},
    Client, Config, ResourceExt,
};
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, info, warn};

pub struct KubeClient {
    client: Client,
}

#[derive(Debug, Clone)]
pub struct CollectionOptions {
    pub include_secrets: bool,
    pub include_custom_resources: bool,
    pub include_events: bool,
    pub include_replicasets: bool,
    pub include_endpoints: bool,
    pub include_leases: bool,
    pub specific_crds: Option<Vec<String>>,
    pub sanitize: bool,
}

impl KubeClient {
    /// Create a new Kubernetes client using the specified kubeconfig file
    pub async fn new_client(kubeconfig_path: &str) -> Result<Self> {
        info!("Loading kubeconfig from: {}", kubeconfig_path);

        // Set the KUBECONFIG environment variable
        std::env::set_var("KUBECONFIG", kubeconfig_path);

        let config = Config::infer().await.context("Failed to load kubeconfig")?;

        let client = Client::try_from(config).context("Failed to create Kubernetes client")?;

        info!("✅ Successfully connected to Kubernetes cluster");
        Ok(KubeClient { client })
    }

    /// List all available namespaces in the cluster
    pub async fn list_namespaces(&self) -> Result<Vec<String>> {
        debug!("Fetching list of namespaces...");

        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        let namespace_list = namespaces
            .list(&Default::default())
            .await
            .context("Failed to list namespaces")?;

        let names: Vec<String> = namespace_list
            .items
            .iter()
            .filter_map(|ns| ns.metadata.name.clone())
            .collect();

        info!("Found {} namespaces", names.len());
        debug!("Namespaces: {:?}", names);
        Ok(names)
    }

    /// Verify that specified namespaces exist
    pub async fn verify_namespaces(&self, requested: &[String]) -> Result<Vec<String>> {
        let available = self.list_namespaces().await?;
        let mut verified = Vec::new();

        for ns in requested {
            if available.contains(ns) {
                verified.push(ns.clone());
            } else {
                warn!("⚠️  Namespace '{}' does not exist, skipping", ns);
            }
        }

        if verified.is_empty() {
            anyhow::bail!("No valid namespaces found");
        }

        Ok(verified)
    }

    /// Collect cluster-scoped resources
    pub async fn collect_cluster_resources(
        &self,
        opts: &CollectionOptions,
    ) -> Result<HashMap<String, Vec<Value>>> {
        let mut resources: HashMap<String, Vec<Value>> = HashMap::new();

        // Discover all API resources
        let discovery = discovery::Discovery::new(self.client.clone()).run().await?;

        // Filter for cluster-scoped resources
        for group in discovery.groups() {
            for (ar, caps) in group.recommended_resources() {
                // Skip if not cluster-scoped
                if caps.scope != Scope::Cluster {
                    continue;
                }

                // Apply filters
                if !self.should_collect_resource(&ar.kind, opts) {
                    debug!("Skipping cluster resource: {}", ar.kind);
                    continue;
                }

                debug!("Collecting cluster resource: {}", ar.kind);

                match self.collect_dynamic_resource(&ar, None, opts).await {
                    Ok(items) if !items.is_empty() => {
                        info!("  ✅ {} ({})", ar.kind, items.len());
                        resources.insert(ar.kind.clone(), items);
                    }
                    Ok(_) => {
                        debug!("  ⏭️  {} (0 items)", ar.kind);
                    }
                    Err(e) => {
                        warn!("  ⚠️  Failed to collect {}: {}", ar.kind, e);
                    }
                }
            }
        }

        Ok(resources)
    }

    /// Collect namespaced resources
    pub async fn collect_namespace_resources(
        &self,
        namespace: &str,
        opts: &CollectionOptions,
    ) -> Result<HashMap<String, Vec<Value>>> {
        let mut resources: HashMap<String, Vec<Value>> = HashMap::new();

        // Discover all API resources
        let discovery = discovery::Discovery::new(self.client.clone()).run().await?;

        // Filter for namespaced resources
        for group in discovery.groups() {
            for (ar, caps) in group.recommended_resources() {
                // Skip if not namespaced
                if caps.scope != Scope::Namespaced {
                    continue;
                }

                // Apply filters
                if !self.should_collect_resource(&ar.kind, opts) {
                    debug!("Skipping namespaced resource: {}", ar.kind);
                    continue;
                }

                debug!("Collecting {} from namespace {}", ar.kind, namespace);

                match self
                    .collect_dynamic_resource(&ar, Some(namespace), opts)
                    .await
                {
                    Ok(items) if !items.is_empty() => {
                        debug!("  ✅ {} ({})", ar.kind, items.len());
                        resources.insert(ar.kind.clone(), items);
                    }
                    Ok(_) => {
                        debug!("  ⏭️  {} (0 items)", ar.kind);
                    }
                    Err(e) => {
                        debug!("  ⚠️  Failed to collect {}: {}", ar.kind, e);
                    }
                }
            }
        }

        Ok(resources)
    }

    /// Collect a dynamic resource type
    async fn collect_dynamic_resource(
        &self,
        ar: &kube::discovery::ApiResource,
        namespace: Option<&str>,
        opts: &CollectionOptions,
    ) -> Result<Vec<Value>> {
        let api: Api<DynamicObject> = if let Some(ns) = namespace {
            Api::namespaced_with(self.client.clone(), ns, &ar.api_version, &ar.kind)
        } else {
            Api::all_with(self.client.clone(), &ar.api_version, &ar.kind)
        };

        let list = api.list(&ListParams::default()).await?;

        let mut items = Vec::new();
        for item in list.items {
            let mut value = serde_json::to_value(&item)?;

            // Sanitize if requested
            if opts.sanitize {
                self.sanitize_resource(&mut value);
            }

            items.push(value);
        }

        Ok(items)
    }

    /// Determine if a resource should be collected based on options
    fn should_collect_resource(&self, kind: &str, opts: &CollectionOptions) -> bool {
        match kind {
            // Always skip these
            "ComponentStatus" | "Binding" => false,

            // Secrets
            "Secret" => opts.include_secrets,

            // Events
            "Event" => opts.include_events,

            // ReplicaSets
            "ReplicaSet" => opts.include_replicasets,

            // Endpoints
            "Endpoints" | "EndpointSlice" => opts.include_endpoints,

            // Leases
            "Lease" => opts.include_leases,

            // Custom Resources - check if it's a CRD
            _ => {
                // If it contains a dot, it's likely a custom resource
                // CRDs are typically named like: mycrd.example.com
                if kind.contains('.') {
                    // Check specific CRDs list if provided
                    if let Some(ref crds) = opts.specific_crds {
                        return crds.iter().any(|crd| kind.eq_ignore_ascii_case(crd));
                    }
                    // Otherwise check general CR flag
                    return opts.include_custom_resources;
                }
                // Standard resource, always collect
                true
            }
        }
    }

    /// Sanitize a resource for kubectl apply readiness
    fn sanitize_resource(&self, value: &mut Value) {
        if let Some(obj) = value.as_object_mut() {
            // Remove metadata fields
            if let Some(metadata) = obj.get_mut("metadata").and_then(|m| m.as_object_mut()) {
                metadata.remove("uid");
                metadata.remove("resourceVersion");
                metadata.remove("selfLink");
                metadata.remove("creationTimestamp");
                metadata.remove("generation");
                metadata.remove("managedFields");
            }

            // Remove status entirely
            obj.remove("status");
        }
    }
}
