use anyhow::Result;
use clap::Parser;
use output::OutputManager;
use tracing::{debug, info};

mod k8s;
mod output;

#[derive(Parser, Debug)]
#[command(name = "ketchup")]
#[command(
    about = "Collects all Kubernetes resources needed to recreate a cluster setup.\nBy default, resources are sanitized for kubectl apply readiness."
)]
#[command(version)]
struct Args {
    /// Path to kubeconfig file (required)
    #[arg(short, long)]
    kubeconfig: String,

    /// Namespaces to collect from (comma-separated, default: all namespaces)
    #[arg(short, long)]
    namespaces: Option<String>,

    /// Output directory for the archive
    #[arg(short, long, default_value = "/tmp")]
    output: String,

    /// Output format: json, yaml, or both
    #[arg(short, long, default_value = "yaml", value_parser = ["json", "yaml", "both"])]
    format: String,

    /// Compression: compressed, uncompressed, or both
    #[arg(short = 'c', long, default_value = "compressed", value_parser = ["compressed", "uncompressed", "both"])]
    compression: String,

    /// Include Secrets (disabled by default for security)
    #[arg(short = 's', long)]
    include_secrets: bool,

    /// Include Custom Resource instances (disabled by default, may show API errors)
    #[arg(short = 'C', long)]
    include_custom_resources: bool,

    /// Include Events (disabled by default, high volume)
    #[arg(short = 'E', long)]
    include_events: bool,

    /// Include ReplicaSets (disabled by default, redundant with Deployments)
    #[arg(short = 'R', long)]
    include_replicasets: bool,

    /// Include Endpoints/EndpointSlices (disabled by default, redundant with Services)
    #[arg(short = 'P', long)]
    include_endpoints: bool,

    /// Include Leases (disabled by default, high churn)
    #[arg(short = 'L', long)]
    include_leases: bool,

    /// Collect specific CRD instances only (comma-separated list of CRD names)
    #[arg(long)]
    crds: Option<String>,

    /// Collect raw unsanitized resources (default: sanitize for kubectl apply readiness)
    #[arg(short = 'r', long)]
    raw: bool,

    /// Verbose logging (progress and summaries)
    #[arg(short, long)]
    verbose: bool,

    /// Debug logging (includes HTTP requests and detailed traces)
    #[arg(short = 'd', long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    init_logging(args.verbose, args.debug);

    info!("🍅 Starting Ketchup - Kubernetes Config Collector");
    info!("Using kubeconfig: {}", args.kubeconfig);

    if !args.raw {
        info!("Resources will be sanitized for kubectl apply readiness (use --raw to disable)");
    }

    // Connect to Kubernetes using specified kubeconfig
    let kube_client = k8s::KubeClient::new_client(&args.kubeconfig).await?;

    // Determine which namespaces to collect from
    let namespaces = if let Some(ns_str) = &args.namespaces {
        let requested: Vec<String> = ns_str.split(',').map(|s| s.trim().to_string()).collect();
        kube_client.verify_namespaces(&requested).await?
    } else {
        info!("No namespaces specified, collecting from all namespaces");
        kube_client.list_namespaces().await?
    };

    info!(
        "Will collect from {} namespace(s): {:?}",
        namespaces.len(),
        namespaces
    );
    info!("Output directory: {}", args.output);

    // Build collection options
    let collection_opts = k8s::CollectionOptions {
        include_secrets: args.include_secrets,
        include_custom_resources: args.include_custom_resources,
        include_events: args.include_events,
        include_replicasets: args.include_replicasets,
        include_endpoints: args.include_endpoints,
        include_leases: args.include_leases,
        specific_crds: args
            .crds
            .as_ref()
            .map(|crds| crds.split(',').map(|s| s.trim().to_string()).collect()),
        sanitize: !args.raw,
    };

    // Log what will be collected
    log_collection_plan(&collection_opts);

    // Create output manager
    let output_manager = OutputManager::new_output_manager(args.output.clone());
    let output_dir = output_manager.create_output_directory()?;

    info!("📦 Collecting cluster-scoped resources...");
    let cluster_resources = kube_client
        .collect_cluster_resources(&collection_opts)
        .await?;

    info!(
        "✅ Collected {} cluster-scoped resource types",
        cluster_resources.len()
    );

    // Save cluster resources
    let cluster_stats =
        output_manager.save_cluster_resources(&output_dir, &cluster_resources, &args.format)?;

    info!("📦 Collecting namespaced resources...");
    let mut namespace_stats = Vec::new();

    for namespace in &namespaces {
        info!("📂 Collecting from namespace: {}", namespace);

        let ns_resources = kube_client
            .collect_namespace_resources(namespace, &collection_opts)
            .await?;

        debug!(
            "Collected {} resource types from namespace {}",
            ns_resources.len(),
            namespace
        );

        let stats = output_manager.save_namespace_resources(
            &output_dir,
            namespace,
            &ns_resources,
            &args.format,
        )?;

        namespace_stats.push((namespace.clone(), stats));
    }

    // Create summary
    output_manager.create_collection_summary(
        &output_dir,
        &cluster_stats,
        &namespace_stats,
        &collection_opts,
    )?;

    // Handle compression
    if let Some(archive_path) = output_manager.handle_compression(&output_dir, &args.compression)? {
        info!("📦 Archive created: {}", archive_path);
    }

    info!("✅ Collection completed successfully!");
    info!("📁 Files saved to: {}", output_dir);

    Ok(())
}

fn init_logging(verbose: bool, debug: bool) {
    let level = if debug {
        tracing::Level::DEBUG
    } else if verbose {
        tracing::Level::INFO
    } else {
        tracing::Level::WARN
    };

    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(debug)
        .init();
}

fn log_collection_plan(opts: &k8s::CollectionOptions) {
    info!("📋 Collection plan:");
    info!("  - Core resources: ✅ Always collected");

    if opts.include_secrets {
        info!("  - Secrets: ✅ Enabled");
    } else {
        debug!("  - Secrets: ⏭️  Skipped (use -s to enable)");
    }

    if opts.include_custom_resources {
        info!("  - Custom Resources: ✅ Enabled");
    } else if let Some(ref crds) = opts.specific_crds {
        info!("  - Custom Resources: ✅ Specific CRDs: {:?}", crds);
    } else {
        debug!("  - Custom Resources: ⏭️  Skipped (use -C to enable)");
    }

    if opts.include_events {
        info!("  - Events: ✅ Enabled");
    } else {
        debug!("  - Events: ⏭️  Skipped (use -E to enable)");
    }

    if opts.include_replicasets {
        info!("  - ReplicaSets: ✅ Enabled");
    } else {
        debug!("  - ReplicaSets: ⏭️  Skipped (use -R to enable)");
    }

    if opts.include_endpoints {
        info!("  - Endpoints: ✅ Enabled");
    } else {
        debug!("  - Endpoints: ⏭️  Skipped (use -P to enable)");
    }

    if opts.include_leases {
        info!("  - Leases: ✅ Enabled");
    } else {
        debug!("  - Leases: ⏭️  Skipped (use -L to enable)");
    }
}
