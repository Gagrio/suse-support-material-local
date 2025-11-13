use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use tracing::{debug, info};

pub struct OutputManager {
    base_dir: String,
    timestamp: DateTime<Utc>,
}

impl OutputManager {
    pub fn new_output_manager(base_dir: String) -> Self {
        Self {
            base_dir,
            timestamp: Utc::now(),
        }
    }

    /// Create timestamped output directory
    pub fn create_output_directory(&self) -> Result<String> {
        let timestamp_str = self.timestamp.format("%Y-%m-%d-%H-%M-%S");
        let output_dir = format!("{}/ketchup-{}", self.base_dir, timestamp_str);

        info!("📁 Creating output directory: {}", output_dir);
        fs::create_dir_all(&output_dir).context("Failed to create output directory")?;

        Ok(output_dir)
    }

    /// Save cluster-scoped resources
    pub fn save_cluster_resources(
        &self,
        output_dir: &str,
        resources: &HashMap<String, Vec<Value>>,
        format: &str,
    ) -> Result<HashMap<String, usize>> {
        let cluster_dir = format!("{}/cluster", output_dir);
        fs::create_dir_all(&cluster_dir).context("Failed to create cluster directory")?;

        let mut stats = HashMap::new();

        for (kind, items) in resources {
            let count = self.save_resource_type(&cluster_dir, kind, items, format)?;
            stats.insert(kind.clone(), count);
        }

        Ok(stats)
    }

    /// Save namespace-scoped resources
    pub fn save_namespace_resources(
        &self,
        output_dir: &str,
        namespace: &str,
        resources: &HashMap<String, Vec<Value>>,
        format: &str,
    ) -> Result<HashMap<String, usize>> {
        let namespace_dir = format!("{}/{}", output_dir, namespace);
        fs::create_dir_all(&namespace_dir).context(format!(
            "Failed to create namespace directory: {}",
            namespace
        ))?;

        let mut stats = HashMap::new();

        for (kind, items) in resources {
            let count = self.save_resource_type(&namespace_dir, kind, items, format)?;
            stats.insert(kind.clone(), count);
        }

        Ok(stats)
    }

    /// Save a specific resource type
    fn save_resource_type(
        &self,
        base_dir: &str,
        kind: &str,
        items: &[Value],
        format: &str,
    ) -> Result<usize> {
        if items.is_empty() {
            return Ok(0);
        }

        // Create resource type directory
        let resource_dir = format!("{}/{}", base_dir, kind.to_lowercase());
        fs::create_dir_all(&resource_dir)
            .context(format!("Failed to create directory for {}", kind))?;

        let mut saved_count = 0;

        for item in items {
            if let Some(name) = item
                .get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|n| n.as_str())
            {
                match format {
                    "json" => {
                        let filename = format!("{}/{}.json", resource_dir, name);
                        let content = serde_json::to_string_pretty(item)?;
                        fs::write(&filename, content)?;
                        saved_count += 1;
                    }
                    "yaml" => {
                        let filename = format!("{}/{}.yaml", resource_dir, name);
                        let content = serde_yaml::to_string(item)?;
                        fs::write(&filename, content)?;
                        saved_count += 1;
                    }
                    "both" => {
                        let json_file = format!("{}/{}.json", resource_dir, name);
                        let yaml_file = format!("{}/{}.yaml", resource_dir, name);

                        let json_content = serde_json::to_string_pretty(item)?;
                        let yaml_content = serde_yaml::to_string(item)?;

                        fs::write(&json_file, json_content)?;
                        fs::write(&yaml_file, yaml_content)?;
                        saved_count += 1;
                    }
                    _ => return Err(anyhow::anyhow!("Invalid format: {}", format)),
                }
            }
        }

        debug!("Saved {} {} resources", saved_count, kind);
        Ok(saved_count)
    }

    /// Create collection summary
    pub fn create_collection_summary(
        &self,
        output_dir: &str,
        cluster_stats: &HashMap<String, usize>,
        namespace_stats: &[(String, HashMap<String, usize>)],
        opts: &crate::k8s::CollectionOptions,
    ) -> Result<()> {
        // Calculate totals
        let total_cluster_resources: usize = cluster_stats.values().sum();
        let mut total_namespaced_resources = 0;
        let mut resource_type_totals: HashMap<String, usize> = HashMap::new();

        // Aggregate namespace stats
        for (_ns, stats) in namespace_stats {
            for (kind, count) in stats {
                total_namespaced_resources += count;
                *resource_type_totals.entry(kind.clone()).or_insert(0) += count;
            }
        }

        // Add cluster stats to resource totals
        for (kind, count) in cluster_stats {
            *resource_type_totals.entry(kind.clone()).or_insert(0) += count;
        }

        // Build namespace details
        let mut namespace_details = serde_json::Map::new();
        for (ns, stats) in namespace_stats {
            let total: usize = stats.values().sum();
            namespace_details.insert(
                ns.clone(),
                serde_json::json!({
                    "total_resources": total,
                    "resource_types": stats,
                }),
            );
        }

        // Build cluster details
        let cluster_details = serde_json::json!({
            "total_resources": total_cluster_resources,
            "resource_types": cluster_stats,
        });

        // Build collection options summary
        let mut collection_flags = Vec::new();
        if opts.include_secrets {
            collection_flags.push("secrets".to_string());
        }
        if opts.include_custom_resources {
            collection_flags.push("custom_resources".to_string());
        }
        if let Some(ref crds) = opts.specific_crds {
            collection_flags.push(format!("specific_crds: {:?}", crds));
        }
        if opts.include_events {
            collection_flags.push("events".to_string());
        }
        if opts.include_replicasets {
            collection_flags.push("replicasets".to_string());
        }
        if opts.include_endpoints {
            collection_flags.push("endpoints".to_string());
        }
        if opts.include_leases {
            collection_flags.push("leases".to_string());
        }

        let summary = serde_json::json!({
            "collection_info": {
                "timestamp": self.timestamp.to_rfc3339(),
                "tool": "ketchup",
                "version": env!("CARGO_PKG_VERSION"),
                "sanitized": opts.sanitize,
                "optional_resources_included": collection_flags,
            },
            "cluster_summary": {
                "total_namespaces": namespace_stats.len(),
                "total_cluster_resources": total_cluster_resources,
                "total_namespaced_resources": total_namespaced_resources,
                "total_resources": total_cluster_resources + total_namespaced_resources,
                "resource_type_counts": resource_type_totals,
            },
            "cluster_resources": cluster_details,
            "namespace_details": namespace_details,
        });

        let filename = format!("{}/collection-summary.yaml", output_dir);
        info!("📊 Creating collection summary: {}", filename);

        let summary_content =
            serde_yaml::to_string(&summary).context("Failed to serialize summary to YAML")?;
        fs::write(&filename, summary_content).context("Failed to write YAML summary file")?;

        Ok(())
    }

    /// Handle compression based on user preference
    pub fn handle_compression(
        &self,
        output_dir: &str,
        compression: &str,
    ) -> Result<Option<String>> {
        match compression {
            "compressed" => {
                let archive_path = self.create_archive(output_dir)?;
                Ok(Some(archive_path))
            }
            "uncompressed" => {
                info!("⏭️  Skipping compression as requested");
                Ok(None)
            }
            "both" => {
                let archive_path = self.create_archive(output_dir)?;
                info!("📦 Files available both compressed and uncompressed");
                Ok(Some(archive_path))
            }
            _ => {
                anyhow::bail!(
                    "Invalid compression: {}. Use compressed, uncompressed, or both",
                    compression
                );
            }
        }
    }

    /// Create compressed archive of the output directory
    fn create_archive(&self, output_dir: &str) -> Result<String> {
        let archive_name = format!("{}.tar.gz", output_dir);
        info!("🗜️  Creating compressed archive: {}", archive_name);

        let tar_gz =
            std::fs::File::create(&archive_name).context("Failed to create archive file")?;
        let enc = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);

        tar.append_dir_all(".", output_dir)
            .context("Failed to add directory to archive")?;
        tar.finish().context("Failed to finalize archive")?;

        Ok(archive_name)
    }
}
