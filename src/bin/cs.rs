use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

#[path = "../hash.rs"]
mod hash;
#[path = "../http.rs"]
mod http;
#[path = "../runtime_data.rs"]
mod runtime_data;
#[path = "../tls.rs"]
mod tls;

#[path = "cs/artifact.rs"]
mod artifact;
#[path = "cs/bundle.rs"]
mod bundle;
#[path = "cs/diagnostic.rs"]
mod diagnostic;
#[path = "cs/project.rs"]
mod project;
#[path = "cs/sbom.rs"]
mod sbom;
#[cfg(test)]
#[path = "cs/tests.rs"]
mod tests;
#[path = "cs/update_package.rs"]
mod update_package;

use artifact::{build_artifact, dry_run_build_artifact, inspect_artifact, run_artifact};
use update_package::build_update_package;

#[derive(Clone, Default, serde::Deserialize)]
struct ProjectManifest {
    #[serde(default)]
    project: ProjectSection,
    #[serde(default)]
    tool: ToolSection,
}

#[derive(Clone, Default, serde::Deserialize)]
struct ProjectSection {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    dynamic: Vec<String>,
}

#[derive(Clone, Default, serde::Deserialize)]
struct ToolSection {
    #[serde(default, rename = "conda-ship")]
    conda_ship: ShipConfig,
}

#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ShipConfig {
    #[serde(default, rename = "runtime-name")]
    runtime_name: Option<String>,
    #[serde(default, rename = "artifact-name")]
    artifact_name: Option<String>,
    #[serde(default, rename = "runtime-version")]
    runtime_version: Option<RuntimeVersionConfig>,
    #[serde(default, rename = "delegate-executable")]
    delegate_executable: Option<String>,
    #[serde(default, rename = "artifact-layout")]
    artifact_layout: Option<BundleLayout>,
    #[serde(default, rename = "exclude-packages")]
    exclude_packages: Vec<String>,
    #[serde(default, rename = "source-environment")]
    source_environment: Option<String>,
    #[serde(default, rename = "docs-url")]
    docs_url: Option<String>,
    #[serde(default, rename = "install-scheme")]
    install_scheme: Option<runtime_data::InstallScheme>,
    #[serde(default, rename = "install-name")]
    install_name: Option<InstallNameConfig>,
    #[serde(default)]
    installer: Option<String>,
    #[serde(default, rename = "condarc-file")]
    condarc_file: Option<PathBuf>,
    #[serde(default, rename = "freeze-base")]
    freeze_base: bool,
    #[serde(default)]
    update: Option<runtime_data::RuntimeUpdateConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
enum RuntimeVersionConfig {
    Value(String),
    Source(RuntimeVersionSourceConfig),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeVersionSourceConfig {
    from: RuntimeVersionSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum RuntimeVersionSource {
    ProjectMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
enum InstallNameConfig {
    Value(String),
    Source(InstallNameSourceConfig),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct InstallNameSourceConfig {
    from: InstallNameSource,
    #[serde(default)]
    base: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum InstallNameSource {
    RuntimeLock,
}

#[derive(Clone, Default)]
struct RuntimeStampConfig {
    channels: Vec<String>,
    packages: Vec<String>,
    exclude_packages: Vec<String>,
    delegate_executable: Option<String>,
    runtime_version: Option<String>,
    runtime_version_source: Option<RuntimeVersionSource>,
    project_dynamic_version: bool,
    docs_url: Option<String>,
    install_scheme: Option<runtime_data::InstallScheme>,
    install_name: Option<String>,
    installer: Option<String>,
    condarc: Option<String>,
    freeze_base: bool,
    update: Option<runtime_data::RuntimeUpdateConfig>,
}

#[derive(clap::Args, Default)]
struct SourceInput {
    /// Exact source manifest to read (sets the project root to its directory)
    #[arg(long)]
    manifest: Option<PathBuf>,

    /// Solved source lockfile to read instead of the manifest's default
    #[arg(long)]
    source_lock: Option<PathBuf>,

    /// Solved environment to ship instead of the manifest's setting
    #[arg(long)]
    source_environment: Option<String>,
}

#[derive(Parser)]
#[command(name = "cs", about = "Build ready-to-run conda runtimes")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build and stage a ready-to-run runtime
    Build {
        /// Artifact layout to produce
        #[arg(long = "artifact-layout", value_enum)]
        artifact_layout: Option<BundleLayout>,

        /// Base runtime identity and default artifact name
        #[arg(long = "runtime-name")]
        runtime_name: Option<String>,

        /// Staged executable and artifact stem
        #[arg(long)]
        artifact_name: Option<String>,

        /// Delegate executable inside the managed prefix
        #[arg(long = "delegate-executable")]
        delegate_executable: Option<String>,

        /// Version shown by the generated runtime
        #[arg(long)]
        runtime_version: Option<String>,

        /// Optional target label appended to staged artifact names
        #[arg(long)]
        target_label: Option<String>,

        /// Conda platform to bundle/describe (default: current)
        #[arg(long)]
        platform: Option<String>,

        /// Target triple used for artifact naming and template selection
        #[arg(long)]
        target: Option<String>,

        /// Prebuilt generic runtime template binary to stamp
        #[arg(long)]
        template: Option<PathBuf>,

        /// Documentation URL stamped into the generated runtime
        #[arg(long)]
        docs_url: Option<String>,

        /// Install scheme stamped into the generated runtime
        #[arg(long = "install-scheme", value_enum)]
        install_scheme: Option<runtime_data::InstallScheme>,

        /// Install name used inside the install scheme
        #[arg(long)]
        install_name: Option<String>,

        /// Package manager or installer that provided the runtime binary
        #[arg(long)]
        installer: Option<String>,

        /// Output directory for staged artifacts
        #[arg(long, default_value = "dist")]
        out_dir: PathBuf,

        /// Preview the build without downloading, stamping, or writing files
        #[arg(long)]
        dry_run: bool,

        /// Project root (default: auto-detect from current directory)
        #[arg(long)]
        root: Option<PathBuf>,

        #[command(flatten)]
        source: SourceInput,
    },

    /// Build and run a staged runtime for local smoke testing
    Run {
        /// Artifact layout to produce before running
        #[arg(long = "artifact-layout", value_enum)]
        artifact_layout: Option<BundleLayout>,

        /// Base runtime identity and default artifact name
        #[arg(long = "runtime-name")]
        runtime_name: Option<String>,

        /// Staged executable and artifact stem
        #[arg(long)]
        artifact_name: Option<String>,

        /// Delegate executable inside the managed prefix
        #[arg(long = "delegate-executable")]
        delegate_executable: Option<String>,

        /// Version shown by the generated runtime
        #[arg(long)]
        runtime_version: Option<String>,

        /// Conda platform to bundle/describe (default: current)
        #[arg(long)]
        platform: Option<String>,

        /// Output directory for staged artifacts
        #[arg(long, default_value = "dist")]
        out_dir: PathBuf,

        /// Managed prefix used by the staged runtime during this smoke test
        #[arg(long)]
        install_path: Option<PathBuf>,

        /// Prebuilt generic runtime template binary to stamp
        #[arg(long)]
        template: Option<PathBuf>,

        /// Documentation URL stamped into the generated runtime
        #[arg(long)]
        docs_url: Option<String>,

        /// Install scheme stamped into the generated runtime
        #[arg(long = "install-scheme", value_enum)]
        install_scheme: Option<runtime_data::InstallScheme>,

        /// Install name used inside the install scheme
        #[arg(long)]
        install_name: Option<String>,

        /// Package manager or installer that provided the runtime binary
        #[arg(long)]
        installer: Option<String>,

        /// Project root (default: auto-detect from current directory)
        #[arg(long)]
        root: Option<PathBuf>,

        /// Arguments passed to the staged runtime
        #[arg(last = true)]
        args: Vec<OsString>,
    },

    /// Inspect project input and derived runtime packages without writing files
    Inspect {
        /// Conda platform to inspect (default: current)
        #[arg(long)]
        platform: Option<String>,

        /// Emit JSON
        #[arg(long)]
        json: bool,

        /// Project root (default: auto-detect from current directory)
        #[arg(long)]
        root: Option<PathBuf>,
    },

    /// Wrap a finalized runtime executable in a conda update package
    PackageUpdate {
        /// Artifact info JSON written by cs build
        #[arg(long)]
        info: PathBuf,

        /// Finalized runtime executable (default: binary recorded in --info)
        #[arg(long)]
        binary: Option<PathBuf>,

        /// Output directory (default: directory containing --info)
        #[arg(long)]
        out_dir: Option<PathBuf>,

        /// Emit package metadata as JSON
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Build { .. } => "build",
            Self::Run { .. } => "run",
            Self::Inspect { .. } => "inspect",
            Self::PackageUpdate { .. } => "package-update",
        }
    }
}

const SHIP_STATE_DIR: &str = "target/conda-ship";
const RUNTIME_LOCK_FILE: &str = "runtime.lock";
const BUNDLE_ARCHIVE_FILE: &str = "bundle.tar.zst";
const RUNTIME_TEMPLATE_ENV: &str = "CONDA_SHIP_TEMPLATE";

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum BundleLayout {
    /// Binary contains lock/metadata; packages download during bootstrap.
    Online,
    /// Binary is paired with a compressed package bundle.
    External,
    /// Binary contains the compressed package bundle.
    Embedded,
}

impl BundleLayout {
    fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::External => "external",
            Self::Embedded => "embedded",
        }
    }

    fn needs_bundle(self) -> bool {
        matches!(self, Self::External | Self::Embedded)
    }
}

fn main() -> ExitCode {
    tls::install_default_provider();

    let cli = Cli::parse();
    let command = cli.command.name();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if diagnostic::structured_errors_requested() {
                diagnostic::print_structured_error(&error, Some(command), 1);
            } else {
                eprintln!("{error:?}");
            }
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> miette::Result<()> {
    match cli.command {
        Command::Build {
            artifact_layout,
            runtime_name,
            artifact_name,
            delegate_executable,
            runtime_version,
            target_label,
            platform,
            target,
            template,
            docs_url,
            install_scheme,
            install_name,
            installer,
            out_dir,
            dry_run,
            root,
            source,
        } => {
            if dry_run {
                dry_run_build_artifact(
                    artifact_layout,
                    runtime_name,
                    artifact_name,
                    delegate_executable,
                    runtime_version,
                    target_label,
                    platform,
                    target,
                    template,
                    docs_url,
                    install_scheme,
                    install_name,
                    installer,
                    out_dir,
                    root,
                    &source,
                )?;
                return Ok(());
            }
            let output = build_artifact(
                artifact_layout,
                runtime_name,
                artifact_name,
                delegate_executable,
                runtime_version,
                target_label,
                platform,
                target,
                template,
                docs_url,
                install_scheme,
                install_name,
                installer,
                out_dir,
                root,
                &source,
            )?;
            eprintln!("metadata {}", output.info.display());
            eprintln!("checksums {}", output.checksums.display());
            eprintln!("lock {}", output.lock.display());
            eprintln!("packages {}", output.package_list.display());
            eprintln!("sbom {}", output.sbom.display());
            if let Some(bundle) = output.bundle {
                eprintln!("bundle {}", bundle.display());
            }
        }
        Command::Run {
            artifact_layout,
            runtime_name,
            artifact_name,
            delegate_executable,
            runtime_version,
            platform,
            out_dir,
            install_path,
            template,
            docs_url,
            install_scheme,
            install_name,
            installer,
            root,
            args,
        } => run_artifact(
            artifact_layout,
            runtime_name,
            artifact_name,
            delegate_executable,
            runtime_version,
            platform,
            out_dir,
            install_path,
            template,
            docs_url,
            install_scheme,
            install_name,
            installer,
            root,
            args,
        )?,
        Command::Inspect {
            platform,
            json,
            root,
        } => inspect_artifact(platform, json, root)?,
        Command::PackageUpdate {
            info,
            binary,
            out_dir,
            json,
        } => build_update_package(&info, binary.as_deref(), out_dir.as_deref(), json)?,
    }
    Ok(())
}
