use std::fmt;

const ERROR_FORMAT_ENV: &str = "CONDA_SHIP_ERROR_FORMAT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticKind {
    CrossBuildRequiresTemplate,
    InvalidArtifactName,
    InvalidPlatform,
    MissingDelegate,
    MissingLockfile,
    MissingManifest,
    MissingProjectRoot,
    MissingRuntime,
    MissingRuntimeVersion,
    MissingRuntimeTemplate,
    MissingSha256,
    MissingSourceEnvironment,
    ProjectMetadataRuntimeVersion,
    RuntimeTemplateIncompatible,
    RuntimeTemplateStamped,
    SourceEnvironmentNotFound,
    Unknown,
}

impl DiagnosticKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::CrossBuildRequiresTemplate => "cross_build_requires_template",
            Self::InvalidArtifactName => "invalid_artifact_name",
            Self::InvalidPlatform => "invalid_platform",
            Self::MissingDelegate => "missing_delegate",
            Self::MissingLockfile => "missing_lockfile",
            Self::MissingManifest => "missing_manifest",
            Self::MissingProjectRoot => "missing_project_root",
            Self::MissingRuntime => "missing_runtime",
            Self::MissingRuntimeVersion => "missing_runtime_version",
            Self::MissingRuntimeTemplate => "missing_runtime_template",
            Self::MissingSha256 => "missing_sha256",
            Self::MissingSourceEnvironment => "missing_source_environment",
            Self::ProjectMetadataRuntimeVersion => "project_metadata_runtime_version",
            Self::RuntimeTemplateIncompatible => "runtime_template_incompatible",
            Self::RuntimeTemplateStamped => "runtime_template_stamped",
            Self::SourceEnvironmentNotFound => "source_environment_not_found",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug)]
pub(crate) struct ShipDiagnostic {
    kind: DiagnosticKind,
    message: String,
    help: Option<String>,
}

impl ShipDiagnostic {
    pub(crate) fn new(
        kind: DiagnosticKind,
        message: impl Into<String>,
        help: Option<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            help,
        }
    }

    pub(crate) fn kind(&self) -> DiagnosticKind {
        self.kind
    }

    pub(crate) fn help_text(&self) -> Option<&str> {
        self.help.as_deref()
    }
}

impl fmt::Display for ShipDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ShipDiagnostic {}

impl miette::Diagnostic for ShipDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(format!("conda_ship::{}", self.kind.as_str())))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|help| Box::new(help) as Box<dyn fmt::Display>)
    }
}

pub(crate) fn ship_error(
    kind: DiagnosticKind,
    message: impl Into<String>,
    help: Option<String>,
) -> miette::Report {
    miette::Report::new(ShipDiagnostic::new(kind, message, help))
}

#[derive(serde::Serialize)]
struct StructuredDiagnostic<'a> {
    schema_version: u8,
    tool: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<&'a str>,
    kind: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
    exit_code: u8,
    causes: Vec<String>,
}

pub(crate) fn structured_errors_requested() -> bool {
    std::env::var(ERROR_FORMAT_ENV).is_ok_and(|value| value.eq_ignore_ascii_case("json"))
}

pub(crate) fn print_structured_error(error: &miette::Report, command: Option<&str>, exit_code: u8) {
    let ship_error = error.downcast_ref::<ShipDiagnostic>();
    let kind = ship_error
        .map(|diagnostic| diagnostic.kind())
        .unwrap_or(DiagnosticKind::Unknown);
    let hint = ship_error
        .and_then(|diagnostic| diagnostic.help_text())
        .map(str::to_string);
    let causes = error.chain().skip(1).map(ToString::to_string).collect();

    let report = StructuredDiagnostic {
        schema_version: 1,
        tool: "cs",
        command,
        kind: kind.as_str(),
        message: error.to_string(),
        hint,
        exit_code,
        causes,
    };

    match serde_json::to_string(&report) {
        Ok(json) => eprintln!("{json}"),
        Err(render_error) => eprintln!(
            r#"{{"schema_version":1,"tool":"cs","kind":"unknown","message":"failed to render structured diagnostic: {render_error}","exit_code":1,"causes":[]}}"#
        ),
    }
}
