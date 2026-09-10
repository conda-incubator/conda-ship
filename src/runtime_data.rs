//! Runtime data stamped onto generated conda-ship artifacts.
//!
//! This module is shared by the builder and runtime binaries. The builder uses
//! the writer path, while the runtime uses the reader path.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use object::endian::LittleEndian;
use object::macho::{self, LinkeditDataCommand, MachHeader64, SegmentCommand64};
use object::pe::{
    self, ImageFileHeader, ImageOptionalHeader32, ImageOptionalHeader64, ImageSectionHeader,
};
use object::read::macho::{MachHeader, Section as _, Segment};
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile};
use object::read::{ReadCache, ReadRef};
use object::{FileKind, bytes_of, from_bytes_mut};
use same_file::Handle as FileIdentity;
use sha2::{Digest, Sha256};

const FOOTER_MAGIC: &[u8; 16] = b"CONDA_SHIP_V0001";
const FORMAT_VERSION: u32 = 1;
const FOOTER_LEN: usize = 8 + 8 + 32 + 32 + 4 + FOOTER_MAGIC.len();
const READER_CAPABILITY_MAGIC: [u8; 16] = *b"CONDA_SHIP_READ1";
const READER_CAPABILITY_VERSION: u32 = 1;
const MACHO_READER_LAYOUT_KIND: u32 = 1;
const PE_READER_LAYOUT_KIND: u32 = 2;
const MACHO_READER_CAPABILITY_SECTION_NAME: [u8; 16] = *b"__cship_reader\0\0";
const PE_READER_CAPABILITY_SECTION_NAME: [u8; pe::IMAGE_SIZEOF_SHORT_NAME] = *b".cscap\0\0";
#[allow(dead_code)]
const MAX_HEADER_LEN: u64 = 16 * 1024 * 1024;
const MAX_MACHO_LOAD_COMMAND_BYTES: usize = 1024 * 1024;
const MACHO_SIGNATURE_ALIGNMENT: u64 = 16;
const MACHO_LC_LAZY_LOAD_DYLIB_INFO: macho::LoadCommandType = macho::LoadCommandType(0x3a);
const PE_CERTIFICATE_ALIGNMENT: u64 = 8;
const MAX_PE_CERTIFICATE_ENTRIES: u64 = 1024;
const MAX_PE_HEADER_BYTES: u64 = 16 * 1024 * 1024;
const PE_MAX_IMAGE_SIZE: u64 = 2 * 1024 * 1024 * 1024;
const PE_ANCHOR_SECTION_NAME: [u8; pe::IMAGE_SIZEOF_SHORT_NAME] = *b".cship\0\0";
const PE_ANCHOR_SECTION_CHARACTERISTICS: pe::SectionFlags =
    pe::SectionFlags(pe::IMAGE_SCN_CNT_INITIALIZED_DATA.0 | pe::IMAGE_SCN_MEM_READ.0);

const MACHO_HEADER_LEN: usize = std::mem::size_of::<MachHeader64<LittleEndian>>();
const MACHO_CODE_SIGNATURE_LEN: usize = std::mem::size_of::<LinkeditDataCommand<LittleEndian>>();

#[repr(C)]
pub(crate) struct ReaderCapabilityRecord {
    magic: [u8; 16],
    record_version: u32,
    footer_version: u32,
    layout_kind: u32,
    flags: u32,
    reserved: [u8; 16],
}

const READER_CAPABILITY_RECORD_LEN: usize = std::mem::size_of::<ReaderCapabilityRecord>();
const _: () = assert!(READER_CAPABILITY_RECORD_LEN == 48);

#[allow(dead_code)]
pub(crate) const fn reader_capability_record() -> ReaderCapabilityRecord {
    ReaderCapabilityRecord {
        magic: READER_CAPABILITY_MAGIC,
        record_version: READER_CAPABILITY_VERSION,
        footer_version: FORMAT_VERSION,
        layout_kind: if cfg!(target_os = "macos") {
            MACHO_READER_LAYOUT_KIND
        } else if cfg!(target_os = "windows") {
            PE_READER_LAYOUT_KIND
        } else {
            0
        },
        flags: 0,
        reserved: [0; 16],
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condarc: Option<String>,
    #[serde(default)]
    pub freeze_base: bool,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateOwnership {
    #[default]
    Direct,
    External,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUpdateConfig {
    pub channel: String,
    pub package: String,
    #[serde(default, rename = "build-number")]
    pub build_number: u64,
    #[serde(default, rename = "ownership", skip_serializing)]
    compatibility_ownership: Option<UpdateOwnership>,
    #[serde(default, rename = "instruction", skip_serializing)]
    compatibility_instruction: Option<String>,
}

#[allow(dead_code)]
impl RuntimeUpdateConfig {
    pub(crate) fn new(channel: String, package: String, build_number: u64) -> Self {
        Self {
            channel,
            package,
            build_number,
            compatibility_ownership: None,
            compatibility_instruction: None,
        }
    }

    pub(crate) fn initial_ownership(&self) -> UpdateOwnership {
        self.compatibility_ownership.unwrap_or_default()
    }

    pub(crate) fn initial_instruction(&self) -> Option<&str> {
        self.compatibility_instruction.as_deref()
    }

    pub(crate) fn has_compatibility_policy(&self) -> bool {
        self.compatibility_ownership.is_some() || self.compatibility_instruction.is_some()
    }

    pub(crate) fn supports_direct_update(&self) -> bool {
        !matches!(
            self.compatibility_ownership,
            Some(UpdateOwnership::External)
        ) && self.compatibility_instruction.is_none()
    }
}

impl RuntimeConfig {
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
            && self.packages.is_empty()
            && self.condarc.is_none()
            && !self.freeze_base
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Eq,
)]
#[serde(rename_all = "kebab-case")]
pub enum InstallScheme {
    #[default]
    CondaHome,
    UserData,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RuntimeDataHeader {
    pub schema_version: u32,
    pub artifact_name: String,
    pub runtime_name: String,
    pub runtime_version: String,
    #[serde(default)]
    pub artifact_layout: String,
    #[serde(default)]
    pub platform: String,
    pub embedded_artifact_name: String,
    pub delegate_executable: String,
    #[serde(default)]
    pub install_scheme: InstallScheme,
    pub install_name: String,
    pub metadata_file: String,
    pub bundle_env_var: String,
    pub offline_env_var: String,
    pub docs_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<RuntimeUpdateConfig>,
    #[serde(default)]
    pub runtime_config: RuntimeConfig,
    #[serde(default)]
    pub runtime_lock: String,
}

impl RuntimeDataHeader {
    pub fn for_name(name: &str) -> Self {
        Self {
            schema_version: FORMAT_VERSION,
            artifact_name: name.to_string(),
            runtime_name: name.to_string(),
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            artifact_layout: String::new(),
            platform: String::new(),
            embedded_artifact_name: name.to_string(),
            delegate_executable: "conda".to_string(),
            install_scheme: InstallScheme::CondaHome,
            install_name: name.to_string(),
            metadata_file: format!(".{name}.json"),
            bundle_env_var: runtime_env_var(name, "BUNDLE"),
            offline_env_var: runtime_env_var(name, "OFFLINE"),
            docs_url: "https://conda-incubator.github.io/conda-ship/".to_string(),
            installer: None,
            update: None,
            runtime_config: RuntimeConfig::default(),
            runtime_lock: String::new(),
        }
    }
}

impl Default for RuntimeDataHeader {
    fn default() -> Self {
        Self::for_name("cs-template")
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct EmbeddedBundle {
    executable: PathBuf,
    offset: u64,
    len: u64,
    sha256: [u8; 32],
}

#[allow(dead_code)]
impl EmbeddedBundle {
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn open_verified(&self) -> io::Result<File> {
        let mut source = File::open(&self.executable)?;
        source.seek(SeekFrom::Start(self.offset))?;
        let mut source = source.take(self.len);
        let mut snapshot = tempfile::tempfile()?;
        let mut hasher = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(read as u64)
                .ok_or_else(|| invalid_data("embedded bundle length overflow"))?;
            hasher.update(&buffer[..read]);
            snapshot.write_all(&buffer[..read])?;
        }
        if copied != self.len {
            return Err(invalid_data(
                "embedded bundle ended before its declared length",
            ));
        }
        let actual = crate::hash::digest_to_array(hasher.finalize());
        if actual != self.sha256 {
            return Err(invalid_data("embedded bundle checksum mismatch"));
        }
        snapshot.seek(SeekFrom::Start(0))?;
        Ok(snapshot)
    }

    pub fn verify(&self) -> io::Result<()> {
        let mut file = File::open(&self.executable)?;
        let actual = hash_file_range(&mut file, self.offset, self.len)?;
        if actual != self.sha256 {
            return Err(invalid_data("embedded bundle checksum mismatch"));
        }
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct RuntimeData {
    pub header: RuntimeDataHeader,
    pub bundle: Option<EmbeddedBundle>,
    pub stamped: bool,
}

#[allow(dead_code)]
static CURRENT_RUNTIME_DATA: LazyLock<RuntimeData> = LazyLock::new(|| match from_current_exe() {
    Ok(Some(data)) => data,
    Ok(None) => RuntimeData::default(),
    Err(err) => {
        eprintln!("error: invalid conda-ship runtime data: {err}");
        std::process::exit(1);
    }
});

#[allow(dead_code)]
pub fn current() -> &'static RuntimeData {
    &CURRENT_RUNTIME_DATA
}

#[allow(dead_code)]
pub fn append_to_binary(
    binary: &Path,
    header: &RuntimeDataHeader,
    bundle: Option<&Path>,
) -> io::Result<()> {
    if header.schema_version != FORMAT_VERSION {
        return Err(invalid_data(format!(
            "unsupported runtime data schema version: {}",
            header.schema_version
        )));
    }

    let header_bytes = serde_json::to_vec(header).map_err(invalid_data)?;
    let header_len = u64::try_from(header_bytes.len())
        .map_err(|_| invalid_data("runtime data header is too large"))?;
    if header_len > MAX_HEADER_LEN {
        return Err(invalid_data(format!(
            "runtime data header is too large: {header_len} bytes"
        )));
    }
    let mut bundle = bundle.map(BundleSource::open).transpose()?;
    let bundle_len = bundle.as_ref().map_or(0, |source| source.len);
    let header_sha256 = crate::hash::digest_to_array(Sha256::digest(&header_bytes));
    let payload_len = header_len
        .checked_add(bundle_len)
        .and_then(|len| len.checked_add(FOOTER_LEN as u64))
        .ok_or_else(|| invalid_data("runtime data length overflow"))?;
    let prepared = PreparedRuntimeData {
        header_bytes,
        header_len,
        bundle_len,
        header_sha256,
        payload_len,
    };

    let input_path_metadata = std::fs::symlink_metadata(binary)?;
    if input_path_metadata.file_type().is_symlink() {
        return Err(invalid_data("runtime template path is a symbolic link"));
    }
    if !input_path_metadata.is_file() {
        return Err(invalid_data("runtime template is not a regular file"));
    }
    let mut input = File::open(binary)?;
    let metadata = input.metadata()?;
    if !path_matches_open_file(binary, &input)? {
        return Err(invalid_data(
            "runtime template path was replaced before it could be opened",
        ));
    }
    let parent = binary
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut input, temporary.as_file_mut())?;
    let copied_metadata = input.metadata()?;
    if copied_metadata.len() != metadata.len()
        || metadata
            .modified()
            .ok()
            .zip(copied_metadata.modified().ok())
            .is_some_and(|(before, after)| before != after)
    {
        return Err(invalid_data("runtime template changed while being read"));
    }
    let copied_sha256 = hash_file_range(temporary.as_file_mut(), 0, metadata.len())?;
    let source_sha256 = hash_file_range(&mut input, 0, metadata.len())?;
    if copied_sha256 != source_sha256 {
        return Err(invalid_data("runtime template changed while being read"));
    }
    temporary
        .as_file()
        .set_permissions(metadata.permissions())?;

    {
        let output = temporary.as_file_mut();
        match detect_binary_format(output)? {
            BinaryFormat::MachO64 => {
                write_macho_runtime_data(output, bundle.as_mut(), &prepared, &header.platform)?
            }
            BinaryFormat::UnsupportedMachO(description) => {
                return Err(invalid_data(format!(
                    "{description} Mach-O binaries are not supported"
                )));
            }
            BinaryFormat::Pe => {
                write_pe_runtime_data(output, bundle.as_mut(), &prepared, &header.platform)?
            }
            BinaryFormat::Other => {
                output.seek(SeekFrom::End(0))?;
                write_runtime_payload(output, bundle.as_mut(), &prepared)?;
            }
        }
        output.flush()?;
    }

    let temporary_path = temporary.path().to_path_buf();
    let written = read_from_file(&temporary_path, temporary.as_file_mut())?
        .ok_or_else(|| invalid_data("completed runtime data stamp could not be read"))?;
    if written.header != *header {
        return Err(invalid_data(
            "completed runtime data stamp does not match its requested header",
        ));
    }
    if let Some(embedded) = written.bundle {
        let actual = hash_file_range(temporary.as_file_mut(), embedded.offset, embedded.len)?;
        if actual != embedded.sha256 {
            return Err(invalid_data("embedded bundle checksum mismatch"));
        }
    } else if prepared.bundle_len != 0 {
        return Err(invalid_data(
            "completed runtime data stamp is missing its embedded bundle",
        ));
    }
    temporary.as_file().sync_all()?;
    if !path_matches_open_file(&temporary_path, temporary.as_file())? {
        return Err(invalid_data(
            "temporary runtime output was replaced before persistence",
        ));
    }
    if !path_matches_open_file(binary, &input)? {
        return Err(invalid_data(
            "runtime template path was replaced while being stamped",
        ));
    }
    let (persisted, temporary_path) = temporary.keep().map_err(|error| error.error)?;
    if let Err(error) = std::fs::rename(&temporary_path, binary) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    if !path_matches_open_file(binary, &persisted)? {
        return Err(invalid_data(
            "persisted runtime output was replaced during persistence",
        ));
    }
    Ok(())
}

fn path_matches_open_file(path: &Path, file: &File) -> io::Result<bool> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Ok(false);
    }
    let path_file = File::open(path)?;
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Ok(false);
    }
    Ok(FileIdentity::from_file(file.try_clone()?)? == FileIdentity::from_file(path_file)?)
}

struct BundleSource {
    file: File,
    len: u64,
    modified: Option<SystemTime>,
}

impl BundleSource {
    fn open(path: &Path) -> io::Result<Self> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(invalid_data("runtime data bundle is not a regular file"));
        }
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid_data("runtime data bundle is not a regular file"));
        }
        Ok(Self {
            file,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }

    fn ensure_unchanged(&self) -> io::Result<()> {
        let metadata = self.file.metadata()?;
        if metadata.len() != self.len
            || self
                .modified
                .zip(metadata.modified().ok())
                .is_some_and(|(before, after)| before != after)
        {
            return Err(invalid_data("runtime data bundle changed while being read"));
        }
        Ok(())
    }
}

struct PreparedRuntimeData {
    header_bytes: Vec<u8>,
    header_len: u64,
    bundle_len: u64,
    header_sha256: [u8; 32],
    payload_len: u64,
}

fn write_runtime_content(
    output: &mut File,
    mut bundle: Option<&mut BundleSource>,
    prepared: &PreparedRuntimeData,
) -> io::Result<[u8; FOOTER_LEN]> {
    output.write_all(&prepared.header_bytes)?;

    let mut bundle_hasher = Sha256::new();
    let mut copied_bundle_len = 0_u64;
    if let Some(source) = bundle.as_deref_mut() {
        source.file.seek(SeekFrom::Start(0))?;
        let mut buf = [0_u8; 64 * 1024];
        loop {
            let read = source.file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            copied_bundle_len = copied_bundle_len
                .checked_add(read as u64)
                .ok_or_else(|| invalid_data("runtime data bundle length overflow"))?;
            if copied_bundle_len > prepared.bundle_len {
                return Err(invalid_data("runtime data bundle grew while being read"));
            }
            bundle_hasher.update(&buf[..read]);
            output.write_all(&buf[..read])?;
        }
    }
    if copied_bundle_len != prepared.bundle_len {
        return Err(invalid_data("runtime data bundle changed while being read"));
    }

    let bundle_sha256 = crate::hash::digest_to_array(bundle_hasher.finalize());
    if let Some(source) = bundle {
        source.ensure_unchanged()?;
        source.file.seek(SeekFrom::Start(0))?;
        let (second_sha256, second_len) =
            crate::hash::sha256_reader((&mut source.file).take(source.len))?;
        source.ensure_unchanged()?;
        if second_len != source.len || second_sha256 != bundle_sha256 {
            return Err(invalid_data("runtime data bundle changed while being read"));
        }
    }
    let footer = encode_footer(
        prepared.header_len,
        prepared.bundle_len,
        prepared.header_sha256,
        bundle_sha256,
    );
    Ok(footer)
}

fn write_runtime_payload(
    output: &mut File,
    bundle: Option<&mut BundleSource>,
    prepared: &PreparedRuntimeData,
) -> io::Result<[u8; FOOTER_LEN]> {
    let footer = write_runtime_content(output, bundle, prepared)?;
    output.write_all(&footer)?;
    Ok(footer)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinaryFormat {
    MachO64,
    UnsupportedMachO(&'static str),
    Pe,
    Other,
}

fn validate_macho_platform(
    cpu_type: macho::CpuType,
    cpu_subtype: macho::CpuSubtype,
    platform: &str,
) -> io::Result<()> {
    let (expected_type, expected_subtype) = match platform {
        "" => return Ok(()),
        "osx-64" => (macho::CPU_TYPE_X86_64, macho::CPU_SUBTYPE_X86_64_ALL),
        "osx-arm64" => (macho::CPU_TYPE_ARM64, macho::CPU_SUBTYPE_ARM64_ALL),
        _ => {
            return Err(invalid_data(format!(
                "Mach-O runtime template does not match platform {platform}"
            )));
        }
    };
    if cpu_type != expected_type || cpu_subtype != expected_subtype.into() {
        return Err(invalid_data(format!(
            "Mach-O runtime template CPU type {cpu_type:#x} with subtype {cpu_subtype:#x} does not match platform {platform}"
        )));
    }
    Ok(())
}

fn validate_pe_platform(machine: pe::Machine, kind: PeKind, platform: &str) -> io::Result<()> {
    let (expected_machine, expected_kind) = match platform {
        "" => return Ok(()),
        "win-32" => (pe::IMAGE_FILE_MACHINE_I386, PeKind::Pe32),
        "win-64" => (pe::IMAGE_FILE_MACHINE_AMD64, PeKind::Pe64),
        "win-arm64" => (pe::IMAGE_FILE_MACHINE_ARM64, PeKind::Pe64),
        _ => {
            return Err(invalid_data(format!(
                "PE runtime template does not match platform {platform}"
            )));
        }
    };
    if machine != expected_machine || kind != expected_kind {
        let image_kind = match kind {
            PeKind::Pe32 => "PE32",
            PeKind::Pe64 => "PE32+",
        };
        return Err(invalid_data(format!(
            "PE runtime template machine {machine:#x} with image kind {image_kind} does not match platform {platform}"
        )));
    }
    Ok(())
}

fn detect_binary_format(file: &mut File) -> io::Result<BinaryFormat> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0_u8; 16];
    let read = file.read(&mut magic)?;
    if read >= 4 {
        match &magic[..4] {
            [0xcf, 0xfa, 0xed, 0xfe] => return Ok(BinaryFormat::MachO64),
            [0xfe, 0xed, 0xfa, 0xcf] => {
                return Ok(BinaryFormat::UnsupportedMachO("big-endian 64-bit"));
            }
            [0xce, 0xfa, 0xed, 0xfe] | [0xfe, 0xed, 0xfa, 0xce] => {
                return Ok(BinaryFormat::UnsupportedMachO("32-bit"));
            }
            [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca] => {
                return Ok(BinaryFormat::UnsupportedMachO("universal"));
            }
            _ => {}
        }
    }
    if magic.get(..2) == Some(b"MZ") {
        let cache = ReadCache::new(file.try_clone()?);
        return match FileKind::parse(&cache).map_err(object_error)? {
            FileKind::Pe32 | FileKind::Pe64 => Ok(BinaryFormat::Pe),
            _ => Err(invalid_data("invalid PE binary")),
        };
    }
    Ok(BinaryFormat::Other)
}

#[allow(dead_code)]
pub(crate) fn validate_runtime_template_reader(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let (location, layout_kind) = match detect_binary_format(&mut file)? {
        BinaryFormat::MachO64 => (
            read_macho_layout(&mut file, file_len)?.reader_capability,
            MACHO_READER_LAYOUT_KIND,
        ),
        BinaryFormat::Pe => (
            read_pe_layout(&file, file_len)?.reader_capability,
            PE_READER_LAYOUT_KIND,
        ),
        _ => return Ok(()),
    };
    validate_reader_capability_record(&mut file, location, layout_kind)
}

fn validate_reader_capability_record(
    file: &mut File,
    location: Option<(u64, u64)>,
    expected_layout_kind: u32,
) -> io::Result<()> {
    let (offset, storage_len) = location.ok_or_else(|| {
        invalid_data("runtime template does not declare the signed-layout reader ABI")
    })?;
    if storage_len < READER_CAPABILITY_RECORD_LEN as u64 {
        return Err(invalid_data(
            "runtime template reader capability record is truncated",
        ));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut record = [0_u8; READER_CAPABILITY_RECORD_LEN];
    file.read_exact(&mut record)?;
    let read_u32 = |start: usize| {
        u32::from_le_bytes(
            record[start..start + 4]
                .try_into()
                .expect("fixed capability field range"),
        )
    };
    if record[..16] != READER_CAPABILITY_MAGIC
        || read_u32(16) != READER_CAPABILITY_VERSION
        || read_u32(20) != FORMAT_VERSION
        || read_u32(24) != expected_layout_kind
        || read_u32(28) != 0
        || record[32..].iter().any(|byte| *byte != 0)
    {
        return Err(invalid_data(
            "runtime template reader capability record is invalid",
        ));
    }
    let mut padding = file.take(storage_len - READER_CAPABILITY_RECORD_LEN as u64);
    let mut buffer = [0_u8; 4096];
    loop {
        let read = padding.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().any(|byte| *byte != 0) {
            return Err(invalid_data(
                "runtime template reader capability section has nonzero padding",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeKind {
    Pe32,
    Pe64,
}

#[derive(Clone, Copy, Debug)]
struct PeAnchorLayout {
    offset: u64,
    raw_size: u64,
    raw_end: u64,
    virtual_end: u64,
}

#[derive(Clone, Copy, Debug)]
struct PeCertificateLayout {
    offset: u64,
    size: u64,
}

#[derive(Debug)]
struct PeImageLayout {
    kind: PeKind,
    machine: pe::Machine,
    file_header_offset: usize,
    optional_header_offset: usize,
    section_table_offset: usize,
    security_directory_offset: Option<usize>,
    section_count: u16,
    size_of_headers: u64,
    size_of_image: u32,
    size_of_initialized_data: u32,
    file_alignment: u64,
    section_alignment: u64,
    first_section_offset: Option<u64>,
    max_raw_end: u64,
    max_virtual_end: u64,
    anchor: Option<PeAnchorLayout>,
    certificate: Option<PeCertificateLayout>,
    reader_capability: Option<(u64, u64)>,
}

fn read_pe_layout(file: &File, file_len: u64) -> io::Result<PeImageLayout> {
    let mut dos = file.try_clone()?;
    dos.seek(SeekFrom::Start(0x3c))?;
    let mut pe_offset = [0_u8; 4];
    dos.read_exact(&mut pe_offset)?;
    let pe_offset = usize::try_from(u32::from_le_bytes(pe_offset))
        .map_err(|_| invalid_data("PE header offset does not fit in memory"))?;

    let cache = ReadCache::new(file.try_clone()?);
    match FileKind::parse(&cache).map_err(object_error)? {
        FileKind::Pe32 => {
            let pe = object::read::pe::PeFile32::parse(&cache).map_err(object_error)?;
            pe_layout_from_file(&pe, PeKind::Pe32, pe_offset, file_len)
        }
        FileKind::Pe64 => {
            let pe = object::read::pe::PeFile64::parse(&cache).map_err(object_error)?;
            pe_layout_from_file(&pe, PeKind::Pe64, pe_offset, file_len)
        }
        _ => Err(invalid_data("invalid PE binary")),
    }
}

fn pe_layout_from_file<'data, Pe, R>(
    pe_file: &PeFile<'data, Pe, R>,
    kind: PeKind,
    pe_offset: usize,
    file_len: u64,
) -> io::Result<PeImageLayout>
where
    Pe: ImageNtHeaders,
    R: ReadRef<'data>,
{
    let nt_headers = pe_file.nt_headers();
    let file_header = nt_headers.file_header();
    let optional = nt_headers.optional_header();
    let characteristics = file_header.characteristics.get(LittleEndian);
    if !characteristics.contains(pe::IMAGE_FILE_EXECUTABLE_IMAGE)
        || characteristics.contains(pe::IMAGE_FILE_DLL)
    {
        return Err(invalid_data(
            "PE runtime template is not an executable image",
        ));
    }
    let section_count = file_header.number_of_sections.get(LittleEndian);
    if section_count == 0 || section_count > 96 {
        return Err(invalid_data("PE section count is outside the loader limit"));
    }
    let optional_header_size = usize::from(file_header.size_of_optional_header.get(LittleEndian));
    let file_header_offset = pe_offset
        .checked_add(4)
        .ok_or_else(|| invalid_data("PE file header offset overflow"))?;
    let optional_header_offset = file_header_offset
        .checked_add(std::mem::size_of::<ImageFileHeader>())
        .ok_or_else(|| invalid_data("PE optional header offset overflow"))?;
    let section_table_offset = optional_header_offset
        .checked_add(optional_header_size)
        .ok_or_else(|| invalid_data("PE section table offset overflow"))?;
    let section_table_end = section_table_offset
        .checked_add(
            usize::from(section_count)
                .checked_mul(pe::IMAGE_SIZEOF_SECTION_HEADER)
                .ok_or_else(|| invalid_data("PE section table size overflow"))?,
        )
        .ok_or_else(|| invalid_data("PE section table size overflow"))?;

    let size_of_headers = u64::from(optional.size_of_headers());
    if size_of_headers > MAX_PE_HEADER_BYTES {
        return Err(invalid_data(format!(
            "PE SizeOfHeaders exceeds the {MAX_PE_HEADER_BYTES}-byte limit"
        )));
    }
    if size_of_headers > file_len || section_table_end as u64 > size_of_headers {
        return Err(invalid_data("PE headers extend past end of file"));
    }
    let file_alignment = u64::from(optional.file_alignment());
    let section_alignment = u64::from(optional.section_alignment());
    let low_alignment = section_alignment < 4 * 1024;
    if file_alignment == 0
        || !file_alignment.is_power_of_two()
        || !(512..=64 * 1024).contains(&file_alignment)
        || section_alignment == 0
        || !section_alignment.is_power_of_two()
        || if low_alignment {
            section_alignment != file_alignment
        } else {
            section_alignment < file_alignment
        }
    {
        return Err(invalid_data("PE section alignment is invalid"));
    }
    if size_of_headers % file_alignment != 0 {
        return Err(invalid_data("PE SizeOfHeaders is not file-aligned"));
    }
    if u64::from(optional.size_of_image()) % section_alignment != 0 {
        return Err(invalid_data("PE SizeOfImage is not section-aligned"));
    }
    if kind == PeKind::Pe64 && u64::from(optional.size_of_image()) > PE_MAX_IMAGE_SIZE {
        return Err(invalid_data(
            "PE32+ SizeOfImage exceeds the 2 GiB loader limit",
        ));
    }

    let optional_fixed_size = match kind {
        PeKind::Pe32 => std::mem::size_of::<ImageOptionalHeader32>(),
        PeKind::Pe64 => std::mem::size_of::<ImageOptionalHeader64>(),
    };
    let security_directory_offset = (optional.number_of_rva_and_sizes()
        > object::pe::IMAGE_DIRECTORY_ENTRY_SECURITY as u32)
        .then(|| {
            optional_header_offset
                .checked_add(optional_fixed_size)
                .and_then(|offset| {
                    offset.checked_add(object::pe::IMAGE_DIRECTORY_ENTRY_SECURITY.checked_mul(8)?)
                })
                .filter(|offset| {
                    offset
                        .checked_add(8)
                        .is_some_and(|end| end <= section_table_offset)
                })
        })
        .flatten();

    let mut first_section_offset = None;
    let mut max_raw_end = size_of_headers;
    let mut max_virtual_end = 0_u64;
    let mut raw_ranges = Vec::new();
    let mut virtual_ranges = Vec::new();
    let mut previous_raw_end = None;
    let mut previous_virtual_end = None;
    let mut anchor = None;
    let mut reader_capability = None;
    for section in pe_file.section_table().iter() {
        let is_anchor = section.name == PE_ANCHOR_SECTION_NAME;
        let is_reader_capability = section.name == PE_READER_CAPABILITY_SECTION_NAME;
        if is_anchor && anchor.is_some() {
            return Err(invalid_data(
                "PE binary has multiple conda-ship anchor sections",
            ));
        }
        let offset = u64::from(section.pointer_to_raw_data.get(LittleEndian));
        let size = u64::from(section.size_of_raw_data.get(LittleEndian));
        let characteristics = section.characteristics.get(LittleEndian);
        let raw_end = offset
            .checked_add(size)
            .ok_or_else(|| invalid_data("PE section file range overflow"))?;
        if raw_end > file_len {
            return Err(invalid_data("PE section extends past end of file"));
        }
        if size != 0 {
            if offset < size_of_headers {
                return Err(invalid_data("PE section overlaps its headers"));
            }
            if offset % file_alignment != 0 || size % file_alignment != 0 {
                return Err(invalid_data("PE raw section is not file-aligned"));
            }
            if low_alignment && offset != u64::from(section.virtual_address.get(LittleEndian)) {
                return Err(invalid_data(
                    "low-alignment PE section file offset does not match its virtual address",
                ));
            }
            if previous_raw_end.is_some_and(|previous| offset < previous) {
                return Err(invalid_data(
                    "PE raw sections overlap or are not ordered by virtual address",
                ));
            }
            previous_raw_end = Some(raw_end);
            raw_ranges.push((offset, raw_end));
            first_section_offset =
                Some(first_section_offset.map_or(offset, |first: u64| first.min(offset)));
            max_raw_end = max_raw_end.max(raw_end);
        }

        let virtual_address = u64::from(section.virtual_address.get(LittleEndian));
        let virtual_size = u64::from(section.virtual_size.get(LittleEndian));
        let virtual_span = virtual_size.max(size);
        let virtual_end = virtual_address
            .checked_add(virtual_span)
            .ok_or_else(|| invalid_data("PE section virtual range overflow"))?;
        if virtual_span != 0 {
            if virtual_address % section_alignment != 0 {
                return Err(invalid_data("PE virtual section is not section-aligned"));
            }
            if let Some(previous) = previous_virtual_end {
                if virtual_address != align_up(previous, section_alignment)? {
                    return Err(invalid_data("PE virtual sections are not adjacent"));
                }
            } else if virtual_address < align_up(size_of_headers, section_alignment)? {
                return Err(invalid_data("PE virtual section overlaps its headers"));
            }
            previous_virtual_end = Some(virtual_end);
            virtual_ranges.push((virtual_address, virtual_end));
            max_virtual_end = max_virtual_end.max(virtual_end);
        }

        if is_anchor {
            let expected_raw_size = align_up(FOOTER_LEN as u64, file_alignment)?;
            if virtual_size != FOOTER_LEN as u64
                || size != expected_raw_size
                || characteristics != PE_ANCHOR_SECTION_CHARACTERISTICS
            {
                return Err(invalid_data(
                    "PE conda-ship anchor section has an invalid layout",
                ));
            }
            anchor = Some(PeAnchorLayout {
                offset,
                raw_size: size,
                raw_end,
                virtual_end,
            });
        }
        if is_reader_capability {
            if reader_capability.is_some() {
                return Err(invalid_data(
                    "PE binary has multiple reader capability sections",
                ));
            }
            let expected_raw_size = align_up(READER_CAPABILITY_RECORD_LEN as u64, file_alignment)?;
            if virtual_size != READER_CAPABILITY_RECORD_LEN as u64
                || size != expected_raw_size
                || characteristics != PE_ANCHOR_SECTION_CHARACTERISTICS
            {
                return Err(invalid_data(
                    "PE reader capability section has an invalid layout",
                ));
            }
            reader_capability = Some((offset, size));
        }
    }
    raw_ranges.sort_unstable_by_key(|range| range.0);
    if raw_ranges
        .windows(2)
        .any(|ranges| ranges[1].0 < ranges[0].1)
    {
        return Err(invalid_data("PE raw sections overlap"));
    }
    virtual_ranges.sort_unstable_by_key(|range| range.0);
    if virtual_ranges
        .windows(2)
        .any(|ranges| ranges[1].0 < ranges[0].1)
    {
        return Err(invalid_data("PE virtual sections overlap"));
    }
    if align_up(max_virtual_end, section_alignment)? > u64::from(optional.size_of_image()) {
        return Err(invalid_data("PE sections extend past SizeOfImage"));
    }
    if let Some(anchor) = anchor
        && (anchor.raw_end != max_raw_end || anchor.virtual_end != max_virtual_end)
    {
        return Err(invalid_data(
            "PE conda-ship anchor is not the final section",
        ));
    }

    let security = pe_file
        .data_directories()
        .iter()
        .nth(object::pe::IMAGE_DIRECTORY_ENTRY_SECURITY)
        .map(object::pe::ImageDataDirectory::address_range)
        .unwrap_or((0, 0));
    let certificate = match security {
        (0, 0) => None,
        (0, _) | (_, 0) => {
            return Err(invalid_data(
                "PE security directory has an incomplete certificate range",
            ));
        }
        (offset, size) => {
            let offset = u64::from(offset);
            let size = u64::from(size);
            if offset % PE_CERTIFICATE_ALIGNMENT != 0 {
                return Err(invalid_data("PE certificate table offset is not aligned"));
            }
            if size < 8 {
                return Err(invalid_data("PE certificate table is too small"));
            }
            let certificate_end = offset
                .checked_add(size)
                .ok_or_else(|| invalid_data("PE certificate table range overflow"))?;
            if certificate_end != file_len {
                return Err(invalid_data(
                    "PE certificate table is not the final data in the file",
                ));
            }
            if offset < max_raw_end {
                return Err(invalid_data("PE certificate table overlaps image data"));
            }
            Some(PeCertificateLayout { offset, size })
        }
    };

    Ok(PeImageLayout {
        kind,
        machine: file_header.machine.get(LittleEndian),
        file_header_offset,
        optional_header_offset,
        section_table_offset,
        security_directory_offset,
        section_count,
        size_of_headers,
        size_of_image: optional.size_of_image(),
        size_of_initialized_data: optional.size_of_initialized_data(),
        file_alignment,
        section_alignment,
        first_section_offset,
        max_raw_end,
        max_virtual_end,
        anchor,
        certificate,
        reader_capability,
    })
}

fn write_pe_runtime_data(
    output: &mut File,
    bundle: Option<&mut BundleSource>,
    prepared: &PreparedRuntimeData,
    platform: &str,
) -> io::Result<()> {
    let file_len = output.metadata()?.len();
    let layout = read_pe_layout(output, file_len)?;
    validate_pe_platform(layout.machine, layout.kind, platform)?;
    if layout.anchor.is_some() {
        return Err(invalid_data(
            "PE runtime template already contains a conda-ship anchor section",
        ));
    }
    if layout.certificate.is_some() {
        return Err(invalid_data(
            "signed PE templates must be unsigned before runtime stamping",
        ));
    }
    if file_len != layout.max_raw_end {
        return Err(invalid_data(
            "PE runtime template has data outside its declared sections",
        ));
    }
    let security_directory_offset = layout.security_directory_offset.ok_or_else(|| {
        invalid_data("PE optional header has no Security Directory entry for signing")
    })?;
    let first_section_offset = layout
        .first_section_offset
        .ok_or_else(|| invalid_data("PE binary has no file-backed sections"))?;
    let new_section_header_offset = layout
        .section_table_offset
        .checked_add(
            usize::from(layout.section_count)
                .checked_mul(pe::IMAGE_SIZEOF_SECTION_HEADER)
                .ok_or_else(|| invalid_data("PE section table size overflow"))?,
        )
        .ok_or_else(|| invalid_data("PE section table offset overflow"))?;
    let new_section_header_end = new_section_header_offset
        .checked_add(pe::IMAGE_SIZEOF_SECTION_HEADER)
        .ok_or_else(|| invalid_data("PE section table size overflow"))?;
    if new_section_header_end > layout.size_of_headers as usize
        || new_section_header_end > first_section_offset as usize
    {
        return Err(invalid_data(
            "PE headers have no room for a runtime anchor section",
        ));
    }

    let anchor_virtual_address = align_up(layout.max_virtual_end, layout.section_alignment)?;
    let anchor_offset = if layout.section_alignment < 4 * 1024 {
        anchor_virtual_address
    } else {
        layout.max_raw_end
    };
    let anchor_raw_size = align_up(FOOTER_LEN as u64, layout.file_alignment)?;
    let anchor_end = anchor_offset
        .checked_add(anchor_raw_size)
        .ok_or_else(|| invalid_data("PE runtime anchor range overflow"))?;
    let content_len = prepared
        .header_len
        .checked_add(prepared.bundle_len)
        .ok_or_else(|| invalid_data("PE runtime data range overflow"))?;
    let (payload_start, payload_end) = pe_runtime_content_layout(anchor_end, content_len)?;
    let anchor_virtual_span = anchor_raw_size.max(FOOTER_LEN as u64);
    let required_size_of_image = align_up(
        anchor_virtual_address
            .checked_add(anchor_virtual_span)
            .ok_or_else(|| invalid_data("PE runtime anchor virtual range overflow"))?,
        layout.section_alignment,
    )?;
    let size_of_image = required_size_of_image.max(u64::from(layout.size_of_image));
    if layout.kind == PeKind::Pe64 && size_of_image > PE_MAX_IMAGE_SIZE {
        return Err(invalid_data(
            "stamped PE32+ SizeOfImage exceeds the 2 GiB loader limit",
        ));
    }

    let anchor_offset_u32 = u32::try_from(anchor_offset)
        .map_err(|_| invalid_data("PE runtime anchor offset does not fit in a section header"))?;
    let anchor_raw_size_u32 = u32::try_from(anchor_raw_size)
        .map_err(|_| invalid_data("PE runtime anchor size does not fit in a section header"))?;
    let anchor_virtual_address_u32 = u32::try_from(anchor_virtual_address)
        .map_err(|_| invalid_data("PE runtime anchor address does not fit in a section header"))?;
    let size_of_image_u32 = u32::try_from(size_of_image)
        .map_err(|_| invalid_data("PE image size does not fit in its optional header"))?;
    let size_of_initialized_data = layout
        .size_of_initialized_data
        .checked_add(anchor_raw_size_u32)
        .ok_or_else(|| invalid_data("PE initialized-data size overflow"))?;
    let section_count = layout
        .section_count
        .checked_add(1)
        .ok_or_else(|| invalid_data("PE section count overflow"))?;
    if section_count > 96 {
        return Err(invalid_data("PE image has no room for another section"));
    }

    let header_size = usize::try_from(layout.size_of_headers)
        .map_err(|_| invalid_data("PE SizeOfHeaders does not fit in memory"))?;
    let mut prefix = Vec::new();
    prefix
        .try_reserve_exact(header_size)
        .map_err(|error| invalid_data(format!("cannot allocate PE header buffer: {error}")))?;
    prefix.resize(header_size, 0);
    output.seek(SeekFrom::Start(0))?;
    output.read_exact(&mut prefix)?;
    if prefix[new_section_header_offset..new_section_header_end]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(invalid_data(
            "PE runtime anchor section header space is not zero-filled",
        ));
    }

    let file_header_bytes = prefix
        .get_mut(layout.file_header_offset..)
        .ok_or_else(|| invalid_data("PE file header is missing"))?;
    let (file_header, _) = from_bytes_mut::<ImageFileHeader>(file_header_bytes)
        .map_err(|_| invalid_data("PE file header is invalid"))?;
    file_header
        .number_of_sections
        .set(LittleEndian, section_count);

    let optional_header_bytes = prefix
        .get_mut(layout.optional_header_offset..)
        .ok_or_else(|| invalid_data("PE optional header is missing"))?;
    match layout.kind {
        PeKind::Pe32 => {
            let (optional, _) = from_bytes_mut::<ImageOptionalHeader32>(optional_header_bytes)
                .map_err(|_| invalid_data("PE32 optional header is invalid"))?;
            optional
                .size_of_initialized_data
                .set(LittleEndian, size_of_initialized_data);
            optional.size_of_image.set(LittleEndian, size_of_image_u32);
            optional.check_sum.set(LittleEndian, 0);
        }
        PeKind::Pe64 => {
            let (optional, _) = from_bytes_mut::<ImageOptionalHeader64>(optional_header_bytes)
                .map_err(|_| invalid_data("PE32+ optional header is invalid"))?;
            optional
                .size_of_initialized_data
                .set(LittleEndian, size_of_initialized_data);
            optional.size_of_image.set(LittleEndian, size_of_image_u32);
            optional.check_sum.set(LittleEndian, 0);
        }
    }
    prefix[security_directory_offset..security_directory_offset + 8].fill(0);

    let mut anchor_section = ImageSectionHeader {
        name: PE_ANCHOR_SECTION_NAME,
        ..ImageSectionHeader::default()
    };
    anchor_section
        .virtual_size
        .set(LittleEndian, FOOTER_LEN as u32);
    anchor_section
        .virtual_address
        .set(LittleEndian, anchor_virtual_address_u32);
    anchor_section
        .size_of_raw_data
        .set(LittleEndian, anchor_raw_size_u32);
    anchor_section
        .pointer_to_raw_data
        .set(LittleEndian, anchor_offset_u32);
    anchor_section
        .characteristics
        .set(LittleEndian, PE_ANCHOR_SECTION_CHARACTERISTICS);
    prefix[new_section_header_offset..new_section_header_end]
        .copy_from_slice(bytes_of(&anchor_section));

    output.set_len(anchor_end)?;
    output.set_len(payload_start)?;
    output.seek(SeekFrom::Start(payload_start))?;
    let footer = write_runtime_content(output, bundle, prepared)?;
    output.set_len(payload_end)?;
    output.seek(SeekFrom::Start(anchor_offset))?;
    output.write_all(&footer)?;
    output.seek(SeekFrom::Start(0))?;
    output.write_all(&prefix)?;
    output.flush()
}

fn pe_runtime_content_layout(anchor_end: u64, content_len: u64) -> io::Result<(u64, u64)> {
    let unaligned_end = anchor_end
        .checked_add(content_len)
        .ok_or_else(|| invalid_data("PE runtime data range overflow"))?;
    let payload_end = align_up(unaligned_end, PE_CERTIFICATE_ALIGNMENT)?;
    u32::try_from(payload_end)
        .map_err(|_| invalid_data("PE runtime data is too large for a certificate table offset"))?;
    let payload_start = anchor_end
        .checked_add(payload_end - unaligned_end)
        .ok_or_else(|| invalid_data("PE runtime data range overflow"))?;
    Ok((payload_start, payload_end))
}

struct MachOLayout {
    header: MachHeader64<LittleEndian>,
    prefix: Vec<u8>,
    cpu_type: macho::CpuType,
    cpu_subtype: macho::CpuSubtype,
    first_section_offset: Option<u64>,
    linkedit_command_offset: usize,
    linkedit_vmaddr: u64,
    linkedit_fileoff: u64,
    linkedit_filesize: u64,
    max_segment_vmaddr: u64,
    max_segment_fileoff: u64,
    max_non_signature_data_end: u64,
    code_signature: Option<(usize, u64, u64)>,
    reader_capability: Option<(u64, u64)>,
}

fn macho_counted_range(
    offset: u32,
    count: u32,
    entry_size: usize,
) -> io::Result<Option<(u64, u64)>> {
    if count == 0 {
        return Ok(None);
    }
    let size = u64::from(count)
        .checked_mul(entry_size as u64)
        .ok_or_else(|| invalid_data("Mach-O table size overflow"))?;
    Ok(Some((u64::from(offset), size)))
}

fn macho_byte_range(offset: u64, size: u64) -> Option<(u64, u64)> {
    (size != 0).then_some((offset, size))
}

fn add_macho_counted_range(
    ranges: &mut Vec<(u64, u64)>,
    offset: u32,
    count: u32,
    entry_size: usize,
) -> io::Result<()> {
    if let Some((offset, size)) = macho_counted_range(offset, count, entry_size)? {
        ranges.push((offset, size));
    }
    Ok(())
}

fn add_macho_byte_range(ranges: &mut Vec<(u64, u64)>, offset: u64, size: u64) {
    if let Some((offset, size)) = macho_byte_range(offset, size) {
        ranges.push((offset, size));
    }
}

fn require_macho_command_size(raw: &[u8], expected: usize) -> io::Result<()> {
    if raw.len() != expected {
        return Err(invalid_data("Mach-O load command has an invalid size"));
    }
    Ok(())
}

fn read_macho_layout(file: &mut File, file_len: u64) -> io::Result<MachOLayout> {
    file.seek(SeekFrom::Start(0))?;
    let mut prefix = vec![0_u8; MACHO_HEADER_LEN];
    file.read_exact(&mut prefix)?;
    let header = *MachHeader64::<LittleEndian>::parse(prefix.as_slice(), 0).map_err(macho_error)?;
    if !header.is_little_endian() {
        return Err(invalid_data("big-endian Mach-O binaries are not supported"));
    }
    if header.filetype(LittleEndian) != macho::MH_EXECUTE {
        return Err(invalid_data(
            "Mach-O runtime template is not an executable image",
        ));
    }
    let header_flags = header.flags(LittleEndian);
    if !header_flags.contains(macho::MH_DYLDLINK | macho::MH_PIE)
        || header_flags.contains(macho::MH_ALLOW_STACK_EXECUTION)
    {
        return Err(invalid_data(
            "Mach-O runtime template has unsupported executable flags",
        ));
    }
    let cpu_type = header.cputype(LittleEndian);
    let cpu_subtype = header.cpusubtype(LittleEndian);
    let expected_subtype = match cpu_type {
        macho::CPU_TYPE_X86_64 => macho::CPU_SUBTYPE_X86_64_ALL.into(),
        macho::CPU_TYPE_ARM64 => macho::CPU_SUBTYPE_ARM64_ALL.into(),
        _ => cpu_subtype,
    };
    if cpu_subtype != expected_subtype {
        return Err(invalid_data(format!(
            "unsupported Mach-O CPU subtype: {cpu_subtype:#x}"
        )));
    }

    let ncmds = usize::try_from(header.ncmds(LittleEndian))
        .map_err(|_| invalid_data("Mach-O load command count does not fit in memory"))?;
    let sizeofcmds = usize::try_from(header.sizeofcmds(LittleEndian))
        .map_err(|_| invalid_data("Mach-O load command size does not fit in memory"))?;
    if sizeofcmds > MAX_MACHO_LOAD_COMMAND_BYTES {
        return Err(invalid_data(format!(
            "Mach-O load command table is too large: {sizeofcmds} bytes"
        )));
    }
    if ncmds > sizeofcmds / std::mem::size_of::<macho::LoadCommand<LittleEndian>>() {
        return Err(invalid_data(
            "Mach-O load command count exceeds its command table",
        ));
    }
    let command_end = MACHO_HEADER_LEN
        .checked_add(sizeofcmds)
        .ok_or_else(|| invalid_data("Mach-O load command size overflow"))?;
    if u64::try_from(command_end)
        .map_err(|_| invalid_data("Mach-O load command size does not fit in a file offset"))?
        > file_len
    {
        return Err(invalid_data("Mach-O load commands extend past end of file"));
    }
    prefix
        .try_reserve_exact(command_end - prefix.len())
        .map_err(|error| invalid_data(format!("cannot allocate Mach-O load commands: {error}")))?;
    prefix.resize(command_end, 0);
    file.read_exact(&mut prefix[MACHO_HEADER_LEN..])?;

    let mut iterator = header
        .load_commands(LittleEndian, prefix.as_slice(), 0)
        .map_err(macho_error)?;
    let mut command_offset = MACHO_HEADER_LEN;
    let mut first_section_offset = None;
    let mut linkedit_command_offset = None;
    let mut linkedit_vmaddr = 0_u64;
    let mut linkedit_fileoff = 0_u64;
    let mut linkedit_filesize = 0_u64;
    let mut max_segment_vmaddr = 0_u64;
    let mut max_segment_fileoff = 0_u64;
    let mut code_signature = None;
    let mut segment_ranges = Vec::new();
    let mut virtual_ranges = Vec::new();
    let mut referenced_ranges = Vec::new();
    let mut executable_section_ranges = Vec::new();
    let mut has_macos_platform_command = false;
    let mut entry_point = None;
    let mut reader_capability = None;
    let mut text_segment_count = 0_u8;
    let mut dylinker_count = 0_u8;

    while let Some(command) = iterator.next().map_err(macho_error)? {
        let raw = command.raw_data();
        if raw.len() % 8 != 0 {
            return Err(invalid_data("Mach-O load command has an invalid size"));
        }
        let next_command_offset = command_offset
            .checked_add(raw.len())
            .ok_or_else(|| invalid_data("Mach-O load command offset overflow"))?;

        if let Some((segment, section_data)) = command.segment_64().map_err(macho_error)? {
            let expected_command_size = std::mem::size_of::<SegmentCommand64<LittleEndian>>()
                .checked_add(
                    usize::try_from(segment.nsects.get(LittleEndian))
                        .map_err(|_| invalid_data("Mach-O section count does not fit in memory"))?
                        .checked_mul(std::mem::size_of::<macho::Section64<LittleEndian>>())
                        .ok_or_else(|| invalid_data("Mach-O section table size overflow"))?,
                )
                .ok_or_else(|| invalid_data("Mach-O segment command size overflow"))?;
            require_macho_command_size(raw, expected_command_size)?;
            let segment_name = segment.name();
            let is_linkedit = segment_name == macho::SEG_LINKEDIT.as_bytes();
            let is_text = segment_name == macho::SEG_TEXT.as_bytes();
            let vmaddr = segment.vmaddr(LittleEndian);
            let vmsize = segment.vmsize(LittleEndian);
            let vm_end = vmaddr
                .checked_add(vmsize)
                .ok_or_else(|| invalid_data("Mach-O segment virtual range overflow"))?;
            if vmsize != 0 {
                virtual_ranges.push((vmaddr, vm_end));
            }
            let (fileoff, filesize) = segment.file_range(LittleEndian);
            let maxprot = segment.maxprot(LittleEndian);
            let initprot = segment.initprot(LittleEndian);
            if !maxprot.contains(initprot) {
                return Err(invalid_data(
                    "Mach-O segment initial protections exceed maximum protections",
                ));
            }
            if (initprot | maxprot).contains(macho::VM_PROT_WRITE | macho::VM_PROT_EXECUTE) {
                return Err(invalid_data(
                    "Mach-O segment permits writable executable memory",
                ));
            }
            if filesize > vmsize {
                return Err(invalid_data(
                    "Mach-O segment file size exceeds its virtual size",
                ));
            }
            let segment_end = fileoff
                .checked_add(filesize)
                .ok_or_else(|| invalid_data("Mach-O segment file range overflow"))?;
            if segment_end > file_len {
                return Err(invalid_data("Mach-O segment extends past end of file"));
            }
            if filesize != 0 {
                segment_ranges.push((fileoff, segment_end));
            }

            if is_text {
                text_segment_count = text_segment_count
                    .checked_add(1)
                    .ok_or_else(|| invalid_data("Mach-O __TEXT segment count overflow"))?;
                if fileoff != 0
                    || !initprot.contains(macho::VM_PROT_READ | macho::VM_PROT_EXECUTE)
                    || initprot.contains(macho::VM_PROT_WRITE)
                    || segment_end < command_end as u64
                {
                    return Err(invalid_data(
                        "Mach-O __TEXT segment has an invalid executable mapping",
                    ));
                }
            }
            if is_linkedit
                && (initprot != macho::VM_PROT_READ
                    || maxprot != macho::VM_PROT_READ
                    || segment.nsects.get(LittleEndian) != 0)
            {
                return Err(invalid_data(
                    "Mach-O __LINKEDIT segment must be read-only and section-free",
                ));
            }

            let sections = segment
                .sections(LittleEndian, section_data)
                .map_err(macho_error)?;
            for section in segment.section_offsets(LittleEndian, sections) {
                let (section, section_offset) = section.map_err(macho_error)?;
                if section.segname != segment.segname {
                    return Err(invalid_data(
                        "Mach-O section segment name does not match its segment",
                    ));
                }
                let section_address = section.addr.get(LittleEndian);
                let section_size = section.size.get(LittleEndian);
                let is_reader_capability = section.sectname == MACHO_READER_CAPABILITY_SECTION_NAME;
                let section_virtual_end = section_address
                    .checked_add(section_size)
                    .ok_or_else(|| invalid_data("Mach-O section virtual range overflow"))?;
                if section_size != 0 && (section_address < vmaddr || section_virtual_end > vm_end) {
                    return Err(invalid_data(
                        "Mach-O section extends outside its virtual segment",
                    ));
                }
                if let Some((offset, size)) = section.file_range(LittleEndian, section_offset) {
                    let section_end = offset
                        .checked_add(size)
                        .ok_or_else(|| invalid_data("Mach-O section file range overflow"))?;
                    if section_end > file_len
                        || (size != 0 && (offset < fileoff || section_end > segment_end))
                    {
                        return Err(invalid_data(
                            "Mach-O section extends outside its file segment",
                        ));
                    }
                    if offset != 0 {
                        first_section_offset = Some(
                            first_section_offset.map_or(offset, |first: u64| first.min(offset)),
                        );
                    }
                    if is_linkedit {
                        add_macho_byte_range(&mut referenced_ranges, offset, size);
                    }
                    if initprot.contains(macho::VM_PROT_EXECUTE) {
                        executable_section_ranges.push((offset, section_end));
                    }
                }
                add_macho_counted_range(
                    &mut referenced_ranges,
                    section.reloff.get(LittleEndian),
                    section.nreloc.get(LittleEndian),
                    std::mem::size_of::<macho::RelocationInfo>(),
                )?;
                if is_reader_capability {
                    if reader_capability.is_some() {
                        return Err(invalid_data(
                            "Mach-O binary has multiple reader capability sections",
                        ));
                    }
                    if !is_text
                        || section_size != READER_CAPABILITY_RECORD_LEN as u64
                        || section.section_type(LittleEndian) != macho::S_REGULAR
                        || section.nreloc.get(LittleEndian) != 0
                        || section.reserved1.get(LittleEndian) != 0
                        || section.reserved2.get(LittleEndian) != 0
                        || section.reserved3.get(LittleEndian) != 0
                    {
                        return Err(invalid_data(
                            "Mach-O reader capability section has an invalid layout",
                        ));
                    }
                    let (offset, size) = section
                        .file_range(LittleEndian, section_offset)
                        .ok_or_else(|| {
                            invalid_data("Mach-O reader capability section has no file data")
                        })?;
                    reader_capability = Some((offset, size));
                }
            }
            max_segment_vmaddr = max_segment_vmaddr.max(vmaddr);
            max_segment_fileoff = max_segment_fileoff.max(fileoff);
            if is_linkedit {
                if linkedit_command_offset.replace(command_offset).is_some() {
                    return Err(invalid_data(
                        "Mach-O binary has multiple __LINKEDIT segments",
                    ));
                }
                linkedit_vmaddr = vmaddr;
                linkedit_fileoff = fileoff;
                linkedit_filesize = filesize;
            }
        }

        match command.cmd() {
            macho::LC_BUILD_VERSION => {
                if has_macos_platform_command {
                    return Err(invalid_data(
                        "Mach-O runtime template has multiple target platform commands",
                    ));
                }
                let version = command
                    .data::<macho::BuildVersionCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                let expected_size = std::mem::size_of::<macho::BuildVersionCommand<LittleEndian>>()
                    .checked_add(
                        usize::try_from(version.ntools.get(LittleEndian))
                            .map_err(|_| {
                                invalid_data("Mach-O build tool count does not fit in memory")
                            })?
                            .checked_mul(
                                std::mem::size_of::<macho::BuildToolVersion<LittleEndian>>(),
                            )
                            .ok_or_else(|| invalid_data("Mach-O build tool size overflow"))?,
                    )
                    .ok_or_else(|| invalid_data("Mach-O build version size overflow"))?;
                require_macho_command_size(raw, expected_size)?;
                let platform = version.platform.get(LittleEndian);
                if platform != macho::PLATFORM_MACOS {
                    return Err(invalid_data(format!(
                        "Mach-O runtime template targets unsupported platform {platform}"
                    )));
                }
                has_macos_platform_command = true;
            }
            macho::LC_VERSION_MIN_MACOSX => {
                if has_macos_platform_command {
                    return Err(invalid_data(
                        "Mach-O runtime template has multiple target platform commands",
                    ));
                }
                command
                    .data::<macho::VersionMinCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(
                    raw,
                    std::mem::size_of::<macho::VersionMinCommand<LittleEndian>>(),
                )?;
                has_macos_platform_command = true;
            }
            macho::LC_VERSION_MIN_IPHONEOS
            | macho::LC_VERSION_MIN_TVOS
            | macho::LC_VERSION_MIN_WATCHOS => {
                return Err(invalid_data(
                    "Mach-O runtime template does not target macOS",
                ));
            }
            macho::LC_MAIN => {
                if entry_point.is_some() {
                    return Err(invalid_data(
                        "Mach-O runtime template has multiple entry points",
                    ));
                }
                let entry = command
                    .data::<macho::EntryPointCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(
                    raw,
                    std::mem::size_of::<macho::EntryPointCommand<LittleEndian>>(),
                )?;
                if entry.stacksize.get(LittleEndian) != 0 {
                    return Err(invalid_data(
                        "Mach-O runtime template requests a custom main stack size",
                    ));
                }
                entry_point = Some(entry.entryoff.get(LittleEndian));
            }
            macho::LC_THREAD | macho::LC_UNIXTHREAD => {
                return Err(invalid_data(
                    "legacy Mach-O thread entry points are not supported",
                ));
            }
            macho::LC_DYLD_INFO | macho::LC_DYLD_INFO_ONLY => {
                let info = command
                    .data::<macho::DyldInfoCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(
                    raw,
                    std::mem::size_of::<macho::DyldInfoCommand<LittleEndian>>(),
                )?;
                for (offset, size) in [
                    (info.rebase_off, info.rebase_size),
                    (info.bind_off, info.bind_size),
                    (info.weak_bind_off, info.weak_bind_size),
                    (info.lazy_bind_off, info.lazy_bind_size),
                    (info.export_off, info.export_size),
                ] {
                    add_macho_byte_range(
                        &mut referenced_ranges,
                        u64::from(offset.get(LittleEndian)),
                        u64::from(size.get(LittleEndian)),
                    );
                }
            }
            macho::LC_SYMTAB => {
                let table = command
                    .data::<macho::SymtabCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(
                    raw,
                    std::mem::size_of::<macho::SymtabCommand<LittleEndian>>(),
                )?;
                add_macho_counted_range(
                    &mut referenced_ranges,
                    table.symoff.get(LittleEndian),
                    table.nsyms.get(LittleEndian),
                    std::mem::size_of::<macho::Nlist64<LittleEndian>>(),
                )?;
                add_macho_byte_range(
                    &mut referenced_ranges,
                    u64::from(table.stroff.get(LittleEndian)),
                    u64::from(table.strsize.get(LittleEndian)),
                );
            }
            macho::LC_DYSYMTAB => {
                let table = command
                    .data::<macho::DysymtabCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(
                    raw,
                    std::mem::size_of::<macho::DysymtabCommand<LittleEndian>>(),
                )?;
                for (offset, count, entry_size) in [
                    (
                        table.tocoff.get(LittleEndian),
                        table.ntoc.get(LittleEndian),
                        std::mem::size_of::<macho::DylibTableOfContents<LittleEndian>>(),
                    ),
                    (
                        table.modtaboff.get(LittleEndian),
                        table.nmodtab.get(LittleEndian),
                        std::mem::size_of::<macho::DylibModule64<LittleEndian>>(),
                    ),
                    (
                        table.extrefsymoff.get(LittleEndian),
                        table.nextrefsyms.get(LittleEndian),
                        std::mem::size_of::<macho::DylibReference<LittleEndian>>(),
                    ),
                    (
                        table.indirectsymoff.get(LittleEndian),
                        table.nindirectsyms.get(LittleEndian),
                        std::mem::size_of::<u32>(),
                    ),
                    (
                        table.extreloff.get(LittleEndian),
                        table.nextrel.get(LittleEndian),
                        std::mem::size_of::<macho::RelocationInfo>(),
                    ),
                    (
                        table.locreloff.get(LittleEndian),
                        table.nlocrel.get(LittleEndian),
                        std::mem::size_of::<macho::RelocationInfo>(),
                    ),
                ] {
                    add_macho_counted_range(&mut referenced_ranges, offset, count, entry_size)?;
                }
            }
            macho::LC_SEGMENT_SPLIT_INFO
            | macho::LC_FUNCTION_STARTS
            | macho::LC_DATA_IN_CODE
            | macho::LC_DYLIB_CODE_SIGN_DRS
            | macho::LC_LINKER_OPTIMIZATION_HINT
            | macho::LC_DYLD_EXPORTS_TRIE
            | macho::LC_DYLD_CHAINED_FIXUPS
            | macho::LC_ATOM_INFO
            | macho::LC_FUNCTION_VARIANTS
            | macho::LC_FUNCTION_VARIANT_FIXUPS
            | MACHO_LC_LAZY_LOAD_DYLIB_INFO => {
                let data = command
                    .data::<LinkeditDataCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(raw, MACHO_CODE_SIGNATURE_LEN)?;
                add_macho_byte_range(
                    &mut referenced_ranges,
                    u64::from(data.dataoff.get(LittleEndian)),
                    u64::from(data.datasize.get(LittleEndian)),
                );
            }
            macho::LC_LOAD_DYLINKER => {
                let linker = command
                    .data::<macho::DylinkerCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                let path = command
                    .string(LittleEndian, linker.name)
                    .map_err(macho_error)?;
                if path != b"/usr/lib/dyld" {
                    return Err(invalid_data(
                        "Mach-O runtime template uses an unsupported dynamic linker",
                    ));
                }
                dylinker_count = dylinker_count
                    .checked_add(1)
                    .ok_or_else(|| invalid_data("Mach-O dynamic linker count overflow"))?;
            }
            macho::LC_LOAD_DYLIB
            | macho::LC_LOAD_WEAK_DYLIB
            | macho::LC_REEXPORT_DYLIB
            | macho::LC_LAZY_LOAD_DYLIB
            | macho::LC_LOAD_UPWARD_DYLIB => {
                let dylib = command
                    .data::<macho::DylibCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                if command
                    .string(LittleEndian, dylib.dylib.name)
                    .map_err(macho_error)?
                    .is_empty()
                {
                    return Err(invalid_data("Mach-O dylib command has an empty path"));
                }
            }
            macho::LC_UUID => {
                command
                    .data::<macho::UuidCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(
                    raw,
                    std::mem::size_of::<macho::UuidCommand<LittleEndian>>(),
                )?;
            }
            macho::LC_SOURCE_VERSION => {
                command
                    .data::<macho::SourceVersionCommand<LittleEndian>>()
                    .map_err(macho_error)?;
                require_macho_command_size(
                    raw,
                    std::mem::size_of::<macho::SourceVersionCommand<LittleEndian>>(),
                )?;
            }
            macho::LC_SEGMENT_64 | macho::LC_CODE_SIGNATURE => {}
            unsupported => {
                return Err(invalid_data(format!(
                    "unsupported Mach-O load command: {unsupported:#x}"
                )));
            }
        }

        if command.cmd() == macho::LC_CODE_SIGNATURE {
            if raw.len() != MACHO_CODE_SIGNATURE_LEN {
                return Err(invalid_data(
                    "Mach-O code signature command has an invalid size",
                ));
            }
            if code_signature.is_some() {
                return Err(invalid_data("Mach-O binary has multiple code signatures"));
            }
            let signature = command
                .data::<LinkeditDataCommand<LittleEndian>>()
                .map_err(macho_error)?;
            code_signature = Some((
                command_offset,
                u64::from(signature.dataoff.get(LittleEndian)),
                u64::from(signature.datasize.get(LittleEndian)),
            ));
        }
        command_offset = next_command_offset;
    }

    if command_offset != command_end {
        return Err(invalid_data(
            "Mach-O load command size does not match header",
        ));
    }
    if !has_macos_platform_command {
        return Err(invalid_data(
            "Mach-O runtime template has no macOS target platform command",
        ));
    }
    if text_segment_count != 1 {
        return Err(invalid_data(
            "Mach-O runtime template must have exactly one __TEXT segment",
        ));
    }
    if dylinker_count != 1 {
        return Err(invalid_data(
            "Mach-O runtime template must load exactly one /usr/lib/dyld",
        ));
    }
    let entry_point = entry_point
        .ok_or_else(|| invalid_data("Mach-O runtime template has no LC_MAIN entry point"))?;
    if !executable_section_ranges
        .iter()
        .any(|(start, end)| *start <= entry_point && entry_point < *end)
    {
        return Err(invalid_data(
            "Mach-O LC_MAIN entry point is outside executable file data",
        ));
    }
    segment_ranges.sort_unstable_by_key(|range| range.0);
    if segment_ranges
        .windows(2)
        .any(|ranges| ranges[1].0 < ranges[0].1)
    {
        return Err(invalid_data("Mach-O file segments overlap"));
    }
    virtual_ranges.sort_unstable_by_key(|range| range.0);
    if virtual_ranges
        .windows(2)
        .any(|ranges| ranges[1].0 < ranges[0].1)
    {
        return Err(invalid_data("Mach-O virtual segments overlap"));
    }
    let linkedit_command_offset = linkedit_command_offset
        .ok_or_else(|| invalid_data("Mach-O binary has no __LINKEDIT segment"))?;
    let linkedit_end = linkedit_fileoff
        .checked_add(linkedit_filesize)
        .ok_or_else(|| invalid_data("Mach-O __LINKEDIT range overflow"))?;
    let mut max_non_signature_data_end = 0_u64;
    for (offset, size) in referenced_ranges {
        let end = offset
            .checked_add(size)
            .ok_or_else(|| invalid_data("Mach-O referenced file range overflow"))?;
        if end > file_len {
            return Err(invalid_data(
                "Mach-O load command references data past end of file",
            ));
        }
        if offset < linkedit_fileoff || end > linkedit_end {
            return Err(invalid_data(
                "Mach-O link-edit data extends outside __LINKEDIT",
            ));
        }
        max_non_signature_data_end = max_non_signature_data_end.max(end);
    }
    if first_section_offset.is_some_and(|offset| offset < command_end as u64 || offset > file_len) {
        return Err(invalid_data("Mach-O load commands overlap section data"));
    }
    Ok(MachOLayout {
        header,
        prefix,
        cpu_type,
        cpu_subtype,
        first_section_offset,
        linkedit_command_offset,
        linkedit_vmaddr,
        linkedit_fileoff,
        linkedit_filesize,
        max_segment_vmaddr,
        max_segment_fileoff,
        max_non_signature_data_end,
        code_signature,
        reader_capability,
    })
}

fn validate_macho_final_layout(layout: &MachOLayout, file_len: u64) -> io::Result<()> {
    let linkedit_end = layout
        .linkedit_fileoff
        .checked_add(layout.linkedit_filesize)
        .ok_or_else(|| invalid_data("Mach-O __LINKEDIT range overflow"))?;
    if linkedit_end != file_len
        || layout.linkedit_fileoff != layout.max_segment_fileoff
        || layout.linkedit_vmaddr != layout.max_segment_vmaddr
    {
        return Err(invalid_data(
            "Mach-O __LINKEDIT segment is not the final file segment",
        ));
    }
    if layout.linkedit_fileoff < layout.prefix.len() as u64 {
        return Err(invalid_data(
            "Mach-O __LINKEDIT segment overlaps its load commands",
        ));
    }
    if let Some((_, offset, size)) = layout.code_signature {
        if size == 0 {
            return Err(invalid_data("Mach-O code signature is empty"));
        }
        let signature_end = offset
            .checked_add(size)
            .ok_or_else(|| invalid_data("Mach-O code signature range overflow"))?;
        if signature_end != file_len {
            return Err(invalid_data(
                "Mach-O code signature is not the final data in the file",
            ));
        }
        if offset < layout.linkedit_fileoff {
            return Err(invalid_data("Mach-O code signature precedes __LINKEDIT"));
        }
        if offset < layout.prefix.len() as u64 {
            return Err(invalid_data(
                "Mach-O code signature overlaps its load commands",
            ));
        }
        if offset % MACHO_SIGNATURE_ALIGNMENT != 0 {
            return Err(invalid_data("Mach-O code signature offset is not aligned"));
        }
        if offset < layout.max_non_signature_data_end {
            return Err(invalid_data(
                "Mach-O code signature overlaps referenced file data",
            ));
        }
    }
    Ok(())
}

fn write_macho_runtime_data(
    output: &mut File,
    bundle: Option<&mut BundleSource>,
    prepared: &PreparedRuntimeData,
    platform: &str,
) -> io::Result<()> {
    // Keep the logical v1 payload unchanged. An existing signature command or
    // verified header room provides the slot that native codesign later uses.
    let file_len = output.metadata()?.len();
    let mut layout = read_macho_layout(output, file_len)?;
    validate_macho_final_layout(&layout, file_len)?;
    validate_macho_platform(layout.cpu_type, layout.cpu_subtype, platform)?;
    let page_alignment = match layout.cpu_type {
        macho::CPU_TYPE_X86_64 => 4 * 1024,
        macho::CPU_TYPE_ARM64 => 16 * 1024,
        cpu_type => {
            return Err(invalid_data(format!(
                "unsupported 64-bit Mach-O CPU type: {cpu_type:#x}"
            )));
        }
    };
    let payload_base = match layout.code_signature {
        Some((command_offset, offset, _)) => {
            let command_end = command_offset
                .checked_add(MACHO_CODE_SIGNATURE_LEN)
                .ok_or_else(|| invalid_data("Mach-O load command offset overflow"))?;
            let freed_start = layout
                .prefix
                .len()
                .checked_sub(MACHO_CODE_SIGNATURE_LEN)
                .ok_or_else(|| invalid_data("Mach-O load command size underflow"))?;
            layout.prefix.copy_within(command_end.., command_offset);
            layout.prefix[freed_start..].fill(0);
            if command_offset < layout.linkedit_command_offset {
                layout.linkedit_command_offset -= MACHO_CODE_SIGNATURE_LEN;
            }
            let command_count = layout
                .header
                .ncmds(LittleEndian)
                .checked_sub(1)
                .ok_or_else(|| invalid_data("Mach-O load command count underflow"))?;
            let commands_len = layout
                .header
                .sizeofcmds(LittleEndian)
                .checked_sub(MACHO_CODE_SIGNATURE_LEN as u32)
                .ok_or_else(|| invalid_data("Mach-O load command size underflow"))?;
            layout.header.ncmds.set(LittleEndian, command_count);
            layout.header.sizeofcmds.set(LittleEndian, commands_len);
            offset
        }
        None => {
            let signed_commands_end = layout
                .prefix
                .len()
                .checked_add(MACHO_CODE_SIGNATURE_LEN)
                .ok_or_else(|| invalid_data("Mach-O load command size overflow"))?;
            if layout
                .first_section_offset
                .is_none_or(|offset| signed_commands_end as u64 > offset)
            {
                return Err(invalid_data(
                    "Mach-O header has no room for a code signature command",
                ));
            }
            ensure_zero_file_range(
                output,
                layout.prefix.len() as u64,
                MACHO_CODE_SIGNATURE_LEN as u64,
                "Mach-O code signature command slot is not zero-filled",
            )?;
            file_len
        }
    };

    let payload_offset = align_up(payload_base, MACHO_SIGNATURE_ALIGNMENT)?;
    let payload_end = payload_offset
        .checked_add(prepared.payload_len)
        .ok_or_else(|| invalid_data("Mach-O runtime data range overflow"))?;
    if align_up(payload_end, MACHO_SIGNATURE_ALIGNMENT)? > u64::from(u32::MAX) {
        return Err(invalid_data(
            "Mach-O runtime data is too large for a code signature offset",
        ));
    }
    let linkedit_filesize = payload_end
        .checked_sub(layout.linkedit_fileoff)
        .ok_or_else(|| invalid_data("Mach-O runtime data precedes __LINKEDIT"))?;
    let linkedit_vmsize = align_up(linkedit_filesize, page_alignment)?;
    let linkedit_bytes = layout
        .prefix
        .get_mut(layout.linkedit_command_offset..)
        .ok_or_else(|| invalid_data("Mach-O __LINKEDIT command is missing"))?;
    let (linkedit, _) = from_bytes_mut::<SegmentCommand64<LittleEndian>>(linkedit_bytes)
        .map_err(|_| invalid_data("Mach-O __LINKEDIT command is invalid"))?;
    linkedit.vmsize.set(LittleEndian, linkedit_vmsize);
    linkedit.filesize.set(LittleEndian, linkedit_filesize);

    layout.prefix[..MACHO_HEADER_LEN].copy_from_slice(bytes_of(&layout.header));

    output.seek(SeekFrom::Start(0))?;
    output.write_all(&layout.prefix)?;
    output.set_len(payload_offset)?;
    output.seek(SeekFrom::Start(payload_offset))?;
    write_runtime_payload(output, bundle, prepared)?;
    output.set_len(payload_end)?;
    output.flush()
}

fn align_up(value: u64, alignment: u64) -> io::Result<u64> {
    if alignment == 0 {
        return Err(invalid_data("binary alignment is zero"));
    }
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned / alignment * alignment)
        .ok_or_else(|| invalid_data("binary alignment overflow"))
}

fn macho_error(error: object::Error) -> io::Error {
    invalid_data(error)
}

fn object_error(error: object::Error) -> io::Error {
    invalid_data(error)
}

#[allow(dead_code)]
pub fn from_current_exe() -> io::Result<Option<RuntimeData>> {
    let exe = std::env::current_exe()?;
    read_from_path(&exe)
}

#[allow(dead_code)]
pub fn read_from_path(path: &Path) -> io::Result<Option<RuntimeData>> {
    let mut file = File::open(path)?;
    read_from_file(path, &mut file)
}

fn read_from_file(path: &Path, file: &mut File) -> io::Result<Option<RuntimeData>> {
    let file_len = file.metadata()?.len();

    let footer_search = match detect_binary_format(file)? {
        BinaryFormat::MachO64 => {
            let (footer_search, cpu_type, cpu_subtype) = macho_footer_search_spec(file, file_len)?;
            let data = read_runtime_data_with_search_spec(path, file, footer_search)?;
            if let Some(data) = &data {
                validate_macho_platform(cpu_type, cpu_subtype, &data.header.platform)?;
            }
            return Ok(data);
        }
        BinaryFormat::UnsupportedMachO(description) => {
            return Err(invalid_data(format!(
                "{description} Mach-O binaries are not supported"
            )));
        }
        BinaryFormat::Pe => return read_pe_runtime_data(path, file, file_len),
        BinaryFormat::Other => FooterSearchSpec {
            data_end: file_len,
            alignment: 1,
            minimum_payload_offset: 0,
        },
    };
    read_runtime_data_with_search_spec(path, file, footer_search)
}

#[derive(Clone, Copy, Debug)]
struct FooterSearchSpec {
    data_end: u64,
    alignment: u64,
    minimum_payload_offset: u64,
}

fn macho_footer_search_spec(
    file: &mut File,
    file_len: u64,
) -> io::Result<(FooterSearchSpec, macho::CpuType, macho::CpuSubtype)> {
    let layout = read_macho_layout(file, file_len)?;
    validate_macho_final_layout(&layout, file_len)?;
    match layout.cpu_type {
        macho::CPU_TYPE_X86_64 | macho::CPU_TYPE_ARM64 => {}
        cpu_type => {
            return Err(invalid_data(format!(
                "unsupported 64-bit Mach-O CPU type: {cpu_type:#x}"
            )));
        }
    }
    let (end, alignment) = match layout.code_signature {
        Some((_, offset, _)) => (offset, MACHO_SIGNATURE_ALIGNMENT),
        None => (file_len, 1),
    };
    Ok((
        FooterSearchSpec {
            data_end: end,
            alignment,
            minimum_payload_offset: layout.linkedit_fileoff,
        },
        layout.cpu_type,
        layout.cpu_subtype,
    ))
}

fn read_pe_runtime_data(
    path: &Path,
    file: &mut File,
    file_len: u64,
) -> io::Result<Option<RuntimeData>> {
    let layout = read_pe_layout(file, file_len)?;
    if let Some(certificate) = layout.certificate {
        validate_pe_certificate_table(file, certificate)?;
    }

    let Some(anchor) = layout.anchor else {
        if let Some(certificate) = layout.certificate {
            let legacy_footer_search = FooterSearchSpec {
                data_end: certificate.offset,
                alignment: PE_CERTIFICATE_ALIGNMENT,
                minimum_payload_offset: layout.max_raw_end,
            };
            if footer_magic_exists_with_search_spec(file, legacy_footer_search)? {
                return Err(invalid_data(
                    "signed PE runtime data has no authenticated anchor section",
                ));
            }
            return Ok(None);
        }
        let data = read_runtime_data_with_search_spec(
            path,
            file,
            FooterSearchSpec {
                data_end: file_len,
                alignment: 1,
                minimum_payload_offset: layout.max_raw_end,
            },
        )?;
        if let Some(data) = &data {
            validate_pe_platform(layout.machine, layout.kind, &data.header.platform)?;
        }
        return Ok(data);
    };

    file.seek(SeekFrom::Start(anchor.offset))?;
    let mut footer = [0_u8; FOOTER_LEN];
    file.read_exact(&mut footer)?;
    let decoded = decode_footer(&footer)?
        .ok_or_else(|| invalid_data("PE conda-ship anchor section has no runtime data footer"))?;
    let anchor_padding_len = anchor
        .raw_size
        .checked_sub(FOOTER_LEN as u64)
        .ok_or_else(|| invalid_data("PE conda-ship anchor section is truncated"))?;
    ensure_zero_file_range(
        file,
        anchor.offset + FOOTER_LEN as u64,
        anchor_padding_len,
        "PE conda-ship anchor section padding is not zero",
    )?;

    let content_len = decoded
        .header_len
        .checked_add(decoded.bundle_len)
        .ok_or_else(|| invalid_data("runtime data length overflow"))?;
    let (payload_start, payload_end) = pe_runtime_content_layout(anchor.raw_end, content_len)?;
    let expected_end = layout
        .certificate
        .map_or(file_len, |certificate| certificate.offset);
    if payload_end != expected_end {
        return Err(invalid_data(
            "PE runtime data does not end at the certificate table offset",
        ));
    }
    ensure_zero_file_range(
        file,
        anchor.raw_end,
        payload_start - anchor.raw_end,
        "PE runtime data alignment padding is not zero",
    )?;
    let data = read_runtime_data_payload(path, file, payload_start, &decoded)?;
    if let Some(data) = &data {
        validate_pe_platform(layout.machine, layout.kind, &data.header.platform)?;
    }
    Ok(data)
}

fn validate_pe_certificate_table(
    file: &mut File,
    certificate: PeCertificateLayout,
) -> io::Result<()> {
    let mut cursor = 0_u64;
    let mut entry_count = 0_u64;
    while cursor < certificate.size {
        if certificate.size - cursor < 8 {
            return Err(invalid_data("PE certificate table entry is truncated"));
        }
        entry_count = entry_count
            .checked_add(1)
            .ok_or_else(|| invalid_data("PE certificate table entry count overflow"))?;
        if entry_count > MAX_PE_CERTIFICATE_ENTRIES {
            return Err(invalid_data(format!(
                "PE certificate table exceeds the {MAX_PE_CERTIFICATE_ENTRIES}-entry limit"
            )));
        }
        file.seek(SeekFrom::Start(certificate.offset + cursor))?;
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let entry_len = u64::from(u32::from_le_bytes(header[..4].try_into().unwrap()));
        if entry_len < 8 {
            return Err(invalid_data("PE certificate table entry is too small"));
        }
        let aligned_len = align_up(entry_len, PE_CERTIFICATE_ALIGNMENT)?;
        cursor = cursor
            .checked_add(aligned_len)
            .ok_or_else(|| invalid_data("PE certificate table entry range overflow"))?;
        if cursor > certificate.size {
            return Err(invalid_data(
                "PE certificate entries exceed the Security Directory size",
            ));
        }
    }
    Ok(())
}

fn footer_magic_exists_with_search_spec(
    file: &mut File,
    footer_search: FooterSearchSpec,
) -> io::Result<bool> {
    for padding in 0..footer_search.alignment {
        let Some(footer_end) = footer_search.data_end.checked_sub(padding) else {
            continue;
        };
        let Some(magic_start) = footer_end.checked_sub(FOOTER_MAGIC.len() as u64) else {
            continue;
        };
        file.seek(SeekFrom::Start(magic_start))?;
        let mut magic = [0_u8; FOOTER_MAGIC.len()];
        file.read_exact(&mut magic)?;
        if magic == *FOOTER_MAGIC {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_zero_file_range(
    file: &mut File,
    offset: u64,
    len: u64,
    message: &'static str,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut remaining = len;
    let mut buffer = [0_u8; 4096];
    while remaining != 0 {
        let read_len = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        file.read_exact(&mut buffer[..read_len])?;
        if buffer[..read_len].iter().any(|byte| *byte != 0) {
            return Err(invalid_data(message));
        }
        remaining -= read_len as u64;
    }
    Ok(())
}

fn read_runtime_data_with_search_spec(
    path: &Path,
    file: &mut File,
    footer_search: FooterSearchSpec,
) -> io::Result<Option<RuntimeData>> {
    let max_padding = footer_search.alignment.saturating_sub(1);
    for padding in 0..=max_padding {
        let Some(footer_end) = footer_search.data_end.checked_sub(padding) else {
            continue;
        };
        let Some(footer_start) = footer_end.checked_sub(FOOTER_LEN as u64) else {
            continue;
        };
        file.seek(SeekFrom::Start(footer_start))?;
        let mut footer = [0_u8; FOOTER_LEN];
        file.read_exact(&mut footer)?;
        if &footer[FOOTER_LEN - FOOTER_MAGIC.len()..] != FOOTER_MAGIC {
            continue;
        }
        if padding != 0 {
            let mut alignment_bytes = [0_u8; MACHO_SIGNATURE_ALIGNMENT as usize - 1];
            file.read_exact(&mut alignment_bytes[..padding as usize])?;
            if alignment_bytes[..padding as usize]
                .iter()
                .any(|byte| *byte != 0)
            {
                return Err(invalid_data(
                    "runtime data footer has nonzero platform alignment padding",
                ));
            }
        }
        return read_runtime_data_at_footer(
            path,
            file,
            footer_start,
            footer_search.minimum_payload_offset,
            &footer,
        );
    }
    Ok(None)
}

fn read_runtime_data_at_footer(
    path: &Path,
    file: &mut File,
    footer_start: u64,
    min_payload_start: u64,
    footer: &[u8; FOOTER_LEN],
) -> io::Result<Option<RuntimeData>> {
    let Some(decoded) = decode_footer(footer)? else {
        return Ok(None);
    };

    let payload_len = decoded
        .header_len
        .checked_add(decoded.bundle_len)
        .ok_or_else(|| invalid_data("runtime data length overflow"))?;
    if payload_len > footer_start {
        return Err(invalid_data(
            "runtime data footer points before start of file",
        ));
    }
    let payload_start = footer_start - payload_len;
    if payload_start < min_payload_start {
        return Err(invalid_data(
            "runtime data payload starts before the platform's minimum payload offset",
        ));
    }

    read_runtime_data_payload(path, file, payload_start, &decoded)
}

fn read_runtime_data_payload(
    path: &Path,
    file: &mut File,
    payload_start: u64,
    decoded: &DecodedFooter,
) -> io::Result<Option<RuntimeData>> {
    if decoded.header_len > MAX_HEADER_LEN {
        return Err(invalid_data(format!(
            "runtime data header is too large: {} bytes",
            decoded.header_len
        )));
    }

    file.seek(SeekFrom::Start(payload_start))?;
    let header_len = usize::try_from(decoded.header_len)
        .map_err(|_| invalid_data("runtime data header does not fit in memory"))?;
    let mut header_bytes = vec![0_u8; header_len];
    file.read_exact(&mut header_bytes)?;
    let actual_header_sha256 = crate::hash::digest_to_array(Sha256::digest(&header_bytes));
    if actual_header_sha256 != decoded.header_sha256 {
        return Err(invalid_data("runtime data header checksum mismatch"));
    }
    let header: RuntimeDataHeader = serde_json::from_slice(&header_bytes).map_err(invalid_data)?;
    if header.schema_version != FORMAT_VERSION {
        return Err(invalid_data(format!(
            "unsupported runtime data schema version: {}",
            header.schema_version
        )));
    }

    let bundle = (decoded.bundle_len > 0).then(|| EmbeddedBundle {
        executable: path.to_path_buf(),
        offset: payload_start + decoded.header_len,
        len: decoded.bundle_len,
        sha256: decoded.bundle_sha256,
    });

    Ok(Some(RuntimeData {
        header,
        bundle,
        stamped: true,
    }))
}

#[allow(dead_code)]
struct DecodedFooter {
    header_len: u64,
    bundle_len: u64,
    header_sha256: [u8; 32],
    bundle_sha256: [u8; 32],
}

#[allow(dead_code)]
fn encode_footer(
    header_len: u64,
    bundle_len: u64,
    header_sha256: [u8; 32],
    bundle_sha256: [u8; 32],
) -> [u8; FOOTER_LEN] {
    let mut footer = [0_u8; FOOTER_LEN];
    footer[0..8].copy_from_slice(&header_len.to_le_bytes());
    footer[8..16].copy_from_slice(&bundle_len.to_le_bytes());
    footer[16..48].copy_from_slice(&header_sha256);
    footer[48..80].copy_from_slice(&bundle_sha256);
    footer[80..84].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    footer[84..100].copy_from_slice(FOOTER_MAGIC);
    footer
}

#[allow(dead_code)]
fn decode_footer(footer: &[u8; FOOTER_LEN]) -> io::Result<Option<DecodedFooter>> {
    if &footer[84..100] != FOOTER_MAGIC {
        return Ok(None);
    }

    let version = u32::from_le_bytes(footer[80..84].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(invalid_data(format!(
            "unsupported runtime data footer version: {version}"
        )));
    }

    let mut header_sha256 = [0_u8; 32];
    header_sha256.copy_from_slice(&footer[16..48]);
    let mut bundle_sha256 = [0_u8; 32];
    bundle_sha256.copy_from_slice(&footer[48..80]);

    Ok(Some(DecodedFooter {
        header_len: u64::from_le_bytes(footer[0..8].try_into().unwrap()),
        bundle_len: u64::from_le_bytes(footer[8..16].try_into().unwrap()),
        header_sha256,
        bundle_sha256,
    }))
}

#[allow(dead_code)]
fn hash_file_range(file: &mut File, offset: u64, len: u64) -> io::Result<[u8; 32]> {
    file.seek(SeekFrom::Start(offset))?;
    let (digest, actual_len) = crate::hash::sha256_reader(file.take(len))?;
    if actual_len != len {
        return Err(invalid_data("file range ended before its declared length"));
    }
    Ok(digest)
}

pub(crate) fn runtime_env_var(name: &str, suffix: &str) -> String {
    let prefix: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{prefix}_{suffix}")
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Read;

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn macho_command_offset(bytes: &[u8], wanted: macho::LoadCommandType) -> usize {
        let command_count = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let mut offset = MACHO_HEADER_LEN;
        for _ in 0..command_count {
            let command = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            let command_size =
                u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if command == wanted.0 {
                return offset;
            }
            offset += command_size;
        }
        panic!("Mach-O command {wanted:#x} not found");
    }

    fn runtime_payload(header: &RuntimeDataHeader) -> Vec<u8> {
        let header_bytes = serde_json::to_vec(header).unwrap();
        let header_sha256 = crate::hash::digest_to_array(Sha256::digest(&header_bytes));
        let bundle_sha256 = crate::hash::digest_to_array(Sha256::digest([]));
        let footer = encode_footer(header_bytes.len() as u64, 0, header_sha256, bundle_sha256);
        let mut payload = header_bytes;
        payload.extend_from_slice(&footer);
        payload
    }

    fn reader_capability_bytes(layout_kind: u32) -> [u8; READER_CAPABILITY_RECORD_LEN] {
        let mut record = [0_u8; READER_CAPABILITY_RECORD_LEN];
        record[..16].copy_from_slice(&READER_CAPABILITY_MAGIC);
        write_u32(&mut record, 16, READER_CAPABILITY_VERSION);
        write_u32(&mut record, 20, FORMAT_VERSION);
        write_u32(&mut record, 24, layout_kind);
        record
    }

    fn add_reader_capability(binary: &Path, offset: u64, layout_kind: u32) {
        let mut file = OpenOptions::new().write(true).open(binary).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(&reader_capability_bytes(layout_kind))
            .unwrap();
    }

    fn macho_fixture(cpu_type: macho::CpuType, with_signature: bool) -> tempfile::NamedTempFile {
        const SEGMENT_LEN: usize = std::mem::size_of::<SegmentCommand64<LittleEndian>>();
        const SECTION_LEN: usize = std::mem::size_of::<macho::Section64<LittleEndian>>();
        const BUILD_VERSION_LEN: usize =
            std::mem::size_of::<macho::BuildVersionCommand<LittleEndian>>();
        const ENTRY_POINT_LEN: usize =
            std::mem::size_of::<macho::EntryPointCommand<LittleEndian>>();
        const DYLINKER_LEN: usize = 32;

        let binary = tempfile::NamedTempFile::new().unwrap();
        let mut bytes = vec![0_u8; 0x400];
        bytes[0x300..].fill(0xa5);

        let text_len = SEGMENT_LEN + 2 * SECTION_LEN;
        let commands_len = text_len
            + BUILD_VERSION_LEN
            + ENTRY_POINT_LEN
            + DYLINKER_LEN
            + SEGMENT_LEN
            + usize::from(with_signature) * MACHO_CODE_SIGNATURE_LEN;
        write_u32(&mut bytes, 0, macho::MH_MAGIC_64);
        write_u32(&mut bytes, 4, cpu_type.0);
        write_u32(
            &mut bytes,
            8,
            match cpu_type {
                macho::CPU_TYPE_X86_64 => macho::CPU_SUBTYPE_X86_64_ALL.0,
                macho::CPU_TYPE_ARM64 => macho::CPU_SUBTYPE_ARM64_ALL.0,
                _ => 0,
            },
        );
        write_u32(&mut bytes, 12, macho::MH_EXECUTE.0);
        write_u32(&mut bytes, 16, 5 + u32::from(with_signature));
        write_u32(&mut bytes, 20, commands_len as u32);
        write_u32(&mut bytes, 24, macho::MH_DYLDLINK.0 | macho::MH_PIE.0);

        let text = MACHO_HEADER_LEN;
        write_u32(&mut bytes, text, macho::LC_SEGMENT_64.0);
        write_u32(&mut bytes, text + 4, text_len as u32);
        bytes[text + 8..text + 14].copy_from_slice(b"__TEXT");
        write_u64(&mut bytes, text + 24, 0x1_0000_0000);
        write_u64(&mut bytes, text + 32, 0x1000);
        write_u64(&mut bytes, text + 40, 0);
        write_u64(&mut bytes, text + 48, 0x280);
        write_u32(&mut bytes, text + 56, 5);
        write_u32(&mut bytes, text + 60, 5);
        write_u32(&mut bytes, text + 64, 2);

        let section = text + SEGMENT_LEN;
        bytes[section..section + 6].copy_from_slice(b"__text");
        bytes[section + 16..section + 22].copy_from_slice(b"__TEXT");
        write_u64(&mut bytes, section + 32, 0x1_0000_0200);
        write_u64(&mut bytes, section + 40, 0x40);
        write_u32(&mut bytes, section + 48, 0x200);

        let capability = section + SECTION_LEN;
        bytes[capability..capability + 14].copy_from_slice(b"__cship_reader");
        bytes[capability + 16..capability + 22].copy_from_slice(b"__TEXT");
        write_u64(&mut bytes, capability + 32, 0x1_0000_0240);
        write_u64(
            &mut bytes,
            capability + 40,
            READER_CAPABILITY_RECORD_LEN as u64,
        );
        write_u32(&mut bytes, capability + 48, 0x240);

        let mut cursor = text + text_len;
        write_u32(&mut bytes, cursor, macho::LC_BUILD_VERSION.0);
        write_u32(&mut bytes, cursor + 4, BUILD_VERSION_LEN as u32);
        write_u32(&mut bytes, cursor + 8, macho::PLATFORM_MACOS.0);
        cursor += BUILD_VERSION_LEN;

        write_u32(&mut bytes, cursor, macho::LC_MAIN.0);
        write_u32(&mut bytes, cursor + 4, ENTRY_POINT_LEN as u32);
        write_u64(&mut bytes, cursor + 8, 0x200);
        cursor += ENTRY_POINT_LEN;

        write_u32(&mut bytes, cursor, macho::LC_LOAD_DYLINKER.0);
        write_u32(&mut bytes, cursor + 4, DYLINKER_LEN as u32);
        write_u32(
            &mut bytes,
            cursor + 8,
            std::mem::size_of::<macho::DylinkerCommand<LittleEndian>>() as u32,
        );
        bytes[cursor + 12..cursor + 26].copy_from_slice(b"/usr/lib/dyld\0");
        cursor += DYLINKER_LEN;

        if with_signature {
            write_u32(&mut bytes, cursor, macho::LC_CODE_SIGNATURE.0);
            write_u32(&mut bytes, cursor + 4, MACHO_CODE_SIGNATURE_LEN as u32);
            write_u32(&mut bytes, cursor + 8, 0x300);
            write_u32(&mut bytes, cursor + 12, 0x100);
            cursor += MACHO_CODE_SIGNATURE_LEN;
        }

        let linkedit = cursor;
        write_u32(&mut bytes, linkedit, macho::LC_SEGMENT_64.0);
        write_u32(&mut bytes, linkedit + 4, SEGMENT_LEN as u32);
        bytes[linkedit + 8..linkedit + 18].copy_from_slice(b"__LINKEDIT");
        write_u64(&mut bytes, linkedit + 24, 0x1_0000_1000);
        write_u64(&mut bytes, linkedit + 32, 0x1000);
        write_u64(&mut bytes, linkedit + 40, 0x280);
        write_u64(&mut bytes, linkedit + 48, 0x180);
        write_u32(&mut bytes, linkedit + 56, 1);
        write_u32(&mut bytes, linkedit + 60, 1);

        assert_eq!(linkedit + SEGMENT_LEN, MACHO_HEADER_LEN + commands_len);
        std::fs::write(binary.path(), bytes).unwrap();
        binary
    }

    fn add_macho_signature(binary: &Path, signature_prefix: &[u8], signature_len: u64) -> u64 {
        assert!(signature_len >= signature_prefix.len() as u64);
        let mut file = File::open(binary).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_macho_layout(&mut file, file_len).unwrap();
        assert!(layout.code_signature.is_none());
        let signature_offset = align_up(file_len, MACHO_SIGNATURE_ALIGNMENT).unwrap();
        let signature_end = signature_offset.checked_add(signature_len).unwrap();
        assert!(signature_end <= u64::from(u32::MAX));

        let mut bytes = std::fs::read(binary).unwrap();
        let command_offset = layout.prefix.len();
        assert!(
            layout
                .first_section_offset
                .is_some_and(
                    |offset| command_offset as u64 + MACHO_CODE_SIGNATURE_LEN as u64 <= offset
                )
        );
        write_u32(
            &mut bytes,
            16,
            layout.header.ncmds(LittleEndian).checked_add(1).unwrap(),
        );
        write_u32(
            &mut bytes,
            20,
            layout
                .header
                .sizeofcmds(LittleEndian)
                .checked_add(MACHO_CODE_SIGNATURE_LEN as u32)
                .unwrap(),
        );
        write_u32(&mut bytes, command_offset, macho::LC_CODE_SIGNATURE.0);
        write_u32(
            &mut bytes,
            command_offset + 4,
            MACHO_CODE_SIGNATURE_LEN as u32,
        );
        write_u32(&mut bytes, command_offset + 8, signature_offset as u32);
        write_u32(&mut bytes, command_offset + 12, signature_len as u32);

        let linkedit_filesize = signature_end - layout.linkedit_fileoff;
        let page_alignment = match layout.cpu_type {
            macho::CPU_TYPE_X86_64 => 4 * 1024,
            macho::CPU_TYPE_ARM64 => 16 * 1024,
            _ => unreachable!(),
        };
        write_u64(
            &mut bytes,
            layout.linkedit_command_offset + 32,
            align_up(linkedit_filesize, page_alignment).unwrap(),
        );
        write_u64(
            &mut bytes,
            layout.linkedit_command_offset + 48,
            linkedit_filesize,
        );
        bytes.resize(signature_offset as usize, 0);
        bytes.extend_from_slice(signature_prefix);
        std::fs::write(binary, bytes).unwrap();
        OpenOptions::new()
            .write(true)
            .open(binary)
            .unwrap()
            .set_len(signature_end)
            .unwrap();
        signature_offset
    }

    fn add_macho_linkedit_command_and_signature(
        binary: &Path,
        command: macho::LoadCommandType,
        data_offset: u32,
        data_size: u32,
    ) {
        let mut bytes = std::fs::read(binary).unwrap();
        let mut file = File::open(binary).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_macho_layout(&mut file, file_len).unwrap();
        assert!(layout.code_signature.is_none());
        let command_offset = layout.prefix.len();
        assert!(
            layout
                .first_section_offset
                .is_some_and(
                    |offset| command_offset as u64 + 2 * MACHO_CODE_SIGNATURE_LEN as u64 <= offset
                )
        );
        write_u32(&mut bytes, 16, layout.header.ncmds(LittleEndian) + 2);
        write_u32(
            &mut bytes,
            20,
            layout.header.sizeofcmds(LittleEndian) + 2 * MACHO_CODE_SIGNATURE_LEN as u32,
        );
        write_u32(&mut bytes, command_offset, command.0);
        write_u32(
            &mut bytes,
            command_offset + 4,
            MACHO_CODE_SIGNATURE_LEN as u32,
        );
        write_u32(&mut bytes, command_offset + 8, data_offset);
        write_u32(&mut bytes, command_offset + 12, data_size);
        let signature_command = command_offset + MACHO_CODE_SIGNATURE_LEN;
        write_u32(&mut bytes, signature_command, macho::LC_CODE_SIGNATURE.0);
        write_u32(
            &mut bytes,
            signature_command + 4,
            MACHO_CODE_SIGNATURE_LEN as u32,
        );
        write_u32(&mut bytes, signature_command + 8, 0x300);
        write_u32(&mut bytes, signature_command + 12, 0x100);
        std::fs::write(binary, bytes).unwrap();
    }

    fn pe_fixture(is_64: bool) -> tempfile::NamedTempFile {
        let binary = tempfile::NamedTempFile::new().unwrap();
        let pe_offset = 0x80;
        let optional_offset = pe_offset + 24;
        let optional_size = if is_64 { 240 } else { 224 };
        let section_offset = optional_offset + optional_size;
        let mut bytes = vec![0_u8; 0x400];
        bytes[..2].copy_from_slice(b"MZ");
        write_u32(&mut bytes, 0x3c, pe_offset as u32);
        write_u32(&mut bytes, pe_offset, object::pe::IMAGE_NT_SIGNATURE);
        write_u16(
            &mut bytes,
            pe_offset + 4,
            if is_64 {
                object::pe::IMAGE_FILE_MACHINE_AMD64.0
            } else {
                object::pe::IMAGE_FILE_MACHINE_I386.0
            },
        );
        write_u16(&mut bytes, pe_offset + 6, 1);
        write_u16(&mut bytes, pe_offset + 20, optional_size as u16);
        write_u16(
            &mut bytes,
            pe_offset + 22,
            object::pe::IMAGE_FILE_EXECUTABLE_IMAGE.0,
        );
        write_u16(
            &mut bytes,
            optional_offset,
            if is_64 {
                object::pe::IMAGE_NT_OPTIONAL_HDR64_MAGIC
            } else {
                object::pe::IMAGE_NT_OPTIONAL_HDR32_MAGIC
            },
        );
        write_u32(&mut bytes, optional_offset + 32, 0x1000);
        write_u32(&mut bytes, optional_offset + 36, 0x200);
        write_u32(&mut bytes, optional_offset + 56, 0x2000);
        write_u32(&mut bytes, optional_offset + 60, 0x200);
        write_u16(
            &mut bytes,
            optional_offset + 68,
            object::pe::IMAGE_SUBSYSTEM_WINDOWS_CUI.0,
        );
        write_u32(
            &mut bytes,
            optional_offset + if is_64 { 108 } else { 92 },
            object::pe::IMAGE_NUMBEROF_DIRECTORY_ENTRIES as u32,
        );

        bytes[section_offset..section_offset + 5].copy_from_slice(b".text");
        write_u32(&mut bytes, section_offset + 8, 0x100);
        write_u32(&mut bytes, section_offset + 12, 0x1000);
        write_u32(&mut bytes, section_offset + 16, 0x200);
        write_u32(&mut bytes, section_offset + 20, 0x200);
        write_u32(&mut bytes, section_offset + 36, 0x6000_0020);
        bytes[0x200..].fill(0xa5);
        std::fs::write(binary.path(), bytes).unwrap();
        binary
    }

    fn add_pe_reader_capability_section(binary: &Path) {
        let mut bytes = std::fs::read(binary).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        let section = pe_section_table_offset(&bytes);
        let capability = section + pe::IMAGE_SIZEOF_SECTION_HEADER;
        write_u16(&mut bytes, pe_offset + 6, 2);
        write_u32(&mut bytes, optional + 56, 0x3000);
        bytes[capability..capability + 8].copy_from_slice(&PE_READER_CAPABILITY_SECTION_NAME);
        write_u32(
            &mut bytes,
            capability + 8,
            READER_CAPABILITY_RECORD_LEN as u32,
        );
        write_u32(&mut bytes, capability + 12, 0x2000);
        write_u32(&mut bytes, capability + 16, 0x200);
        write_u32(&mut bytes, capability + 20, 0x400);
        write_u32(
            &mut bytes,
            capability + 36,
            PE_ANCHOR_SECTION_CHARACTERISTICS.0,
        );
        bytes.resize(0x600, 0);
        bytes[0x400..0x400 + READER_CAPABILITY_RECORD_LEN]
            .copy_from_slice(&reader_capability_bytes(PE_READER_LAYOUT_KIND));
        std::fs::write(binary, bytes).unwrap();
    }

    fn pe_security_directory_offset(bytes: &[u8]) -> usize {
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional_offset = pe_offset + 24;
        let magic = u16::from_le_bytes(
            bytes[optional_offset..optional_offset + 2]
                .try_into()
                .unwrap(),
        );
        optional_offset
            + match magic {
                object::pe::IMAGE_NT_OPTIONAL_HDR32_MAGIC => 96,
                object::pe::IMAGE_NT_OPTIONAL_HDR64_MAGIC => 112,
                _ => panic!("unexpected PE optional header magic"),
            }
            + object::pe::IMAGE_DIRECTORY_ENTRY_SECURITY * 8
    }

    fn pe_section_table_offset(bytes: &[u8]) -> usize {
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional_size =
            u16::from_le_bytes(bytes[pe_offset + 20..pe_offset + 22].try_into().unwrap()) as usize;
        pe_offset + 24 + optional_size
    }

    fn add_pe_certificate(binary: &Path, certificate_payload: &[u8]) -> u64 {
        let mut bytes = std::fs::read(binary).unwrap();
        let certificate_offset = bytes.len() as u64;
        assert_eq!(certificate_offset % PE_CERTIFICATE_ALIGNMENT, 0);
        let unaligned_size = 8_u64 + certificate_payload.len() as u64;
        let certificate_size = align_up(unaligned_size, PE_CERTIFICATE_ALIGNMENT).unwrap();
        let security_offset = pe_security_directory_offset(&bytes);
        write_u32(&mut bytes, security_offset, certificate_offset as u32);
        write_u32(&mut bytes, security_offset + 4, certificate_size as u32);
        let old_len = bytes.len();
        bytes.resize(old_len + certificate_size as usize, 0);
        write_u32(&mut bytes, old_len, certificate_size as u32);
        write_u16(&mut bytes, old_len + 4, 0x0200);
        write_u16(&mut bytes, old_len + 6, 0x0002);
        bytes[old_len + 8..old_len + 8 + certificate_payload.len()]
            .copy_from_slice(certificate_payload);
        std::fs::write(binary, bytes).unwrap();
        certificate_offset
    }

    fn add_pe_certificate_entries(binary: &Path, entry_count: u64) {
        assert_ne!(entry_count, 0);
        let mut bytes = std::fs::read(binary).unwrap();
        let certificate_offset = bytes.len() as u64;
        assert_eq!(certificate_offset % PE_CERTIFICATE_ALIGNMENT, 0);
        let certificate_size = entry_count.checked_mul(PE_CERTIFICATE_ALIGNMENT).unwrap();
        let security_offset = pe_security_directory_offset(&bytes);
        write_u32(
            &mut bytes,
            security_offset,
            u32::try_from(certificate_offset).unwrap(),
        );
        write_u32(
            &mut bytes,
            security_offset + 4,
            u32::try_from(certificate_size).unwrap(),
        );
        let old_len = bytes.len();
        bytes.resize(old_len + usize::try_from(certificate_size).unwrap(), 0);
        for entry_index in 0..entry_count {
            let entry_offset =
                old_len + usize::try_from(entry_index * PE_CERTIFICATE_ALIGNMENT).unwrap();
            write_u32(&mut bytes, entry_offset, PE_CERTIFICATE_ALIGNMENT as u32);
            write_u16(&mut bytes, entry_offset + 4, 0x0200);
            write_u16(&mut bytes, entry_offset + 6, 0x0002);
        }
        std::fs::write(binary, bytes).unwrap();
    }

    #[test]
    fn test_legacy_update_policy_is_read_but_not_written() {
        let update: RuntimeUpdateConfig = serde_json::from_value(serde_json::json!({
            "channel": "https://packages.example.test/runtime",
            "package": "demo-runtime",
            "build-number": 2,
            "ownership": "external",
            "instruction": "Use the installer."
        }))
        .unwrap();

        assert_eq!(update.initial_ownership(), UpdateOwnership::External);
        assert_eq!(update.initial_instruction(), Some("Use the installer."));

        let serialized = serde_json::to_value(update).unwrap();
        assert!(serialized.get("ownership").is_none());
        assert!(serialized.get("instruction").is_none());
    }

    #[test]
    fn test_legacy_external_update_policy_is_not_direct_capable() {
        let direct: RuntimeUpdateConfig = serde_json::from_value(serde_json::json!({
            "channel": "https://packages.example.test/runtime",
            "package": "demo-runtime",
            "ownership": "direct"
        }))
        .unwrap();
        let external: RuntimeUpdateConfig = serde_json::from_value(serde_json::json!({
            "channel": "https://packages.example.test/runtime",
            "package": "demo-runtime",
            "ownership": "external"
        }))
        .unwrap();
        let malformed_direct: RuntimeUpdateConfig = serde_json::from_value(serde_json::json!({
            "channel": "https://packages.example.test/runtime",
            "package": "demo-runtime",
            "instruction": "Use an external installer."
        }))
        .unwrap();
        let current = RuntimeUpdateConfig::new(
            "https://packages.example.test/runtime".to_string(),
            "demo-runtime".to_string(),
            0,
        );

        assert!(direct.supports_direct_update());
        assert!(current.supports_direct_update());
        assert!(!external.supports_direct_update());
        assert!(!malformed_direct.supports_direct_update());
    }

    #[test]
    fn test_missing_runtime_data_returns_none() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"plain binary").unwrap();

        let data = read_from_path(tmp.path()).unwrap();
        assert!(data.is_none());
    }

    #[test]
    fn test_append_and_read_runtime_data_without_bundle() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"binary").unwrap();

        let mut header = RuntimeDataHeader::for_name("snek");
        header.runtime_lock = "lock data".to_string();
        header.runtime_config.channels = vec!["conda-forge".to_string()];
        header.runtime_config.condarc = Some("channels: []\n".to_string());
        header.runtime_config.freeze_base = true;

        append_to_binary(tmp.path(), &header, None).unwrap();
        let data = read_from_path(tmp.path()).unwrap().unwrap();

        assert_eq!(data.header.artifact_name, "snek");
        assert_eq!(data.header.runtime_name, "snek");
        assert_eq!(data.header.delegate_executable, "conda");
        assert_eq!(data.header.install_scheme, InstallScheme::CondaHome);
        assert_eq!(data.header.install_name, "snek");
        assert_eq!(data.header.runtime_lock, "lock data");
        assert_eq!(
            data.header.runtime_config.condarc.as_deref(),
            Some("channels: []\n")
        );
        assert!(data.header.runtime_config.freeze_base);
        assert!(data.bundle.is_none());
        assert!(data.stamped);
    }

    #[test]
    fn test_append_replaces_path_while_original_handle_remains_open() {
        let mut binary = tempfile::NamedTempFile::new().unwrap();
        let binary_path = binary.path().to_path_buf();
        std::fs::write(&binary_path, b"binary").unwrap();
        let header = RuntimeDataHeader::for_name("snek");

        append_to_binary(&binary_path, &header, None).unwrap();

        let mut original = Vec::new();
        binary.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        binary.as_file_mut().read_to_end(&mut original).unwrap();
        assert_eq!(original, b"binary");
        assert_eq!(
            read_from_path(&binary_path).unwrap().unwrap().header,
            header
        );
    }

    #[test]
    fn test_macho_runtime_data_extends_linkedit_and_reuses_signature_slot() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let bundle = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(bundle.path(), b"bundle data").unwrap();
        let mut before = File::open(binary.path()).unwrap();
        let before_len = before.metadata().unwrap().len();
        let before_layout = read_macho_layout(&mut before, before_len).unwrap();
        assert!(
            before_layout.code_signature.unwrap().0 < before_layout.linkedit_command_offset,
            "the fixture keeps a command after LC_CODE_SIGNATURE"
        );

        let mut header = RuntimeDataHeader::for_name("snek");
        header.runtime_lock = "lock data".to_string();
        append_to_binary(binary.path(), &header, Some(bundle.path())).unwrap();

        let bytes = std::fs::read(binary.path()).unwrap();
        assert_eq!(bytes[0x300], b'{');
        let mut file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_macho_layout(&mut file, file_len).unwrap();
        assert!(layout.code_signature.is_none());
        assert_eq!(layout.linkedit_fileoff + layout.linkedit_filesize, file_len);

        let (linkedit, _) = object::from_bytes::<SegmentCommand64<LittleEndian>>(
            &layout.prefix[layout.linkedit_command_offset..],
        )
        .unwrap();
        assert_eq!(
            linkedit.filesize.get(LittleEndian),
            layout.linkedit_filesize
        );
        assert_eq!(linkedit.vmsize.get(LittleEndian) % (16 * 1024), 0);

        let commands_end = MACHO_HEADER_LEN + layout.header.sizeofcmds(LittleEndian) as usize;
        assert_eq!(
            &bytes[commands_end..commands_end + MACHO_CODE_SIGNATURE_LEN],
            &[0_u8; MACHO_CODE_SIGNATURE_LEN]
        );

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header.runtime_name, "snek");
        assert_eq!(data.header.runtime_lock, "lock data");
        data.bundle.unwrap().verify().unwrap();
    }

    #[test]
    fn test_x86_64_macho_linkedit_uses_4k_alignment() {
        let binary = macho_fixture(macho::CPU_TYPE_X86_64, true);
        let header = RuntimeDataHeader::for_name("snek");

        append_to_binary(binary.path(), &header, None).unwrap();

        let mut file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_macho_layout(&mut file, file_len).unwrap();
        let (linkedit, _) = object::from_bytes::<SegmentCommand64<LittleEndian>>(
            &layout.prefix[layout.linkedit_command_offset..],
        )
        .unwrap();
        assert_eq!(linkedit.vmsize.get(LittleEndian) % (4 * 1024), 0);
    }

    #[test]
    fn test_unsigned_macho_uses_existing_signature_command_room() {
        let binary = macho_fixture(macho::CPU_TYPE_X86_64, false);
        let header = RuntimeDataHeader::for_name("snek");

        append_to_binary(binary.path(), &header, None).unwrap();

        let mut file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_macho_layout(&mut file, file_len).unwrap();
        assert!(layout.code_signature.is_none());
        assert_eq!(layout.linkedit_fileoff + layout.linkedit_filesize, file_len);
        assert_eq!(
            read_from_path(binary.path()).unwrap().unwrap().header,
            header
        );
    }

    #[test]
    fn test_unsigned_macho_without_signature_command_room_is_rejected() {
        let binary = macho_fixture(macho::CPU_TYPE_X86_64, false);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let section_offset_field =
            MACHO_HEADER_LEN + std::mem::size_of::<SegmentCommand64<LittleEndian>>() + 48;
        let commands_end =
            MACHO_HEADER_LEN + u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        write_u32(&mut bytes, section_offset_field, commands_end as u32);
        let entry_point = macho_command_offset(&bytes, macho::LC_MAIN);
        write_u64(&mut bytes, entry_point + 8, commands_end as u64);
        std::fs::write(binary.path(), &bytes).unwrap();
        let header = RuntimeDataHeader::for_name("snek");

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("header has no room for a code signature command"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_macho_linkedit_cannot_overlap_load_commands() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_macho_layout(&mut file, file_len).unwrap();
        let signature_command_offset = layout.code_signature.unwrap().0;
        let malicious_offset = layout.prefix.len() as u64 - 8;
        drop(file);

        let mut bytes = std::fs::read(binary.path()).unwrap();
        write_u64(
            &mut bytes,
            layout.linkedit_command_offset + 40,
            malicious_offset,
        );
        write_u64(
            &mut bytes,
            layout.linkedit_command_offset + 48,
            file_len - malicious_offset,
        );
        write_u32(
            &mut bytes,
            signature_command_offset + 8,
            malicious_offset as u32,
        );
        write_u32(
            &mut bytes,
            signature_command_offset + 12,
            (file_len - malicious_offset) as u32,
        );
        std::fs::write(binary.path(), &bytes).unwrap();
        let header = RuntimeDataHeader::for_name("snek");

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(
            error.to_string().contains("segments overlap")
                || error.to_string().contains("overlaps its load commands"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_macho_load_command_count_is_bounded() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        write_u32(&mut bytes, 16, u32::MAX);
        std::fs::write(binary.path(), &bytes).unwrap();
        let header = RuntimeDataHeader::for_name("snek");

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("count exceeds its command table"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_macho_load_command_allocation_is_bounded() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        write_u32(&mut bytes, 20, (MAX_MACHO_LOAD_COMMAND_BYTES + 1) as u32);
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();

        assert!(error.to_string().contains("command table is too large"));
    }

    #[test]
    fn test_macho_writer_rejects_non_executable_template_without_modifying_input() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        write_u32(&mut bytes, 12, macho::MH_DYLIB.0);
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("not-executable"),
            None,
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("not an executable image"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_macho_writer_rejects_non_macos_template_without_modifying_input() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let build_version = macho_command_offset(&bytes, macho::LC_BUILD_VERSION);
        write_u32(&mut bytes, build_version + 8, macho::PLATFORM_IOS.0);
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("ios-template"),
            None,
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("unsupported platform"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_macho_writer_rejects_entry_point_outside_executable_data() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let entry_point = macho_command_offset(&bytes, macho::LC_MAIN);
        write_u64(&mut bytes, entry_point + 8, 0x300);
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("invalid-entry-point"),
            None,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("entry point is outside executable file data"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_macho_writer_rejects_platform_architecture_mismatch() {
        let binary = macho_fixture(macho::CPU_TYPE_X86_64, false);
        let before = std::fs::read(binary.path()).unwrap();
        let mut header = RuntimeDataHeader::for_name("wrong-architecture");
        header.platform = "osx-arm64".to_string();

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(
            error.to_string().contains("does not match platform"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), before);
    }

    #[test]
    fn test_macho_section_range_overflow_is_rejected() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let section = MACHO_HEADER_LEN + std::mem::size_of::<SegmentCommand64<LittleEndian>>();
        write_u64(&mut bytes, section + 40, u64::MAX);
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();

        assert!(error.to_string().contains("section virtual range overflow"));
    }

    #[test]
    fn test_macho_overlapping_virtual_segments_are_rejected() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let mut file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_macho_layout(&mut file, file_len).unwrap();
        drop(file);

        let mut bytes = std::fs::read(binary.path()).unwrap();
        write_u64(
            &mut bytes,
            layout.linkedit_command_offset + 24,
            0x1_0000_0800,
        );
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("virtual segments overlap"));
    }

    #[test]
    fn test_universal_macho_is_not_stamped_as_an_overlay() {
        let binary = tempfile::NamedTempFile::new().unwrap();
        let mut bytes = [0_u8; 16];
        bytes[..4].copy_from_slice(&0xbebafeca_u32.to_le_bytes());
        std::fs::write(binary.path(), bytes).unwrap();
        let header = RuntimeDataHeader::for_name("snek");

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(error.to_string().contains("universal Mach-O"), "{error}");
        assert_eq!(std::fs::metadata(binary.path()).unwrap().len(), 16);
    }

    #[test]
    fn test_truncated_macho_magic_is_not_stamped_as_generic_overlay() {
        let binary = tempfile::NamedTempFile::new().unwrap();
        let bytes = macho::MH_MAGIC_64.to_le_bytes();
        std::fs::write(binary.path(), bytes).unwrap();
        let header = RuntimeDataHeader::for_name("snek");

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(error.to_string().contains("failed to fill whole buffer"));
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_read_macho_runtime_data_before_platform_signature() {
        let binary = macho_fixture(macho::CPU_TYPE_X86_64, false);
        let header = RuntimeDataHeader::for_name("snek");
        append_to_binary(binary.path(), &header, None).unwrap();
        add_macho_signature(binary.path(), b"signature", 128 * 1024);

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header.runtime_name, "snek");
    }

    #[test]
    fn test_macho_reader_ignores_valid_shadow_stamp_inside_signature() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, false);
        let legitimate = RuntimeDataHeader::for_name("legitimate");
        append_to_binary(binary.path(), &legitimate, None).unwrap();
        let forged = runtime_payload(&RuntimeDataHeader::for_name("forged"));
        add_macho_signature(binary.path(), &forged, 64 * 1024);

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header.runtime_name, "legitimate");
    }

    #[test]
    fn test_macho_reader_rejects_corrupt_anchor_instead_of_shadow_stamp() {
        let binary = macho_fixture(macho::CPU_TYPE_X86_64, false);
        let legitimate = RuntimeDataHeader::for_name("legitimate");
        append_to_binary(binary.path(), &legitimate, None).unwrap();
        let footer_end = std::fs::metadata(binary.path()).unwrap().len();
        let forged = runtime_payload(&RuntimeDataHeader::for_name("forged"));
        add_macho_signature(binary.path(), &forged, 64 * 1024);

        let mut file = OpenOptions::new().write(true).open(binary.path()).unwrap();
        file.seek(SeekFrom::Start(footer_end - FOOTER_LEN as u64 - 1))
            .unwrap();
        file.write_all(b"!").unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("header checksum mismatch"));
    }

    #[test]
    fn test_macho_reader_accepts_signature_larger_than_16_mib() {
        let binary = macho_fixture(macho::CPU_TYPE_X86_64, false);
        let header = RuntimeDataHeader::for_name("large-signature");
        append_to_binary(binary.path(), &header, None).unwrap();
        add_macho_signature(binary.path(), b"signature", 17 * 1024 * 1024);

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header, header);
    }

    #[test]
    fn test_macho_signature_cannot_overlap_referenced_linkedit_data() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, false);
        add_macho_linkedit_command_and_signature(
            binary.path(),
            macho::LC_FUNCTION_STARTS,
            0x300,
            0x20,
        );
        let before = std::fs::read(binary.path()).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("overlapping-signature"),
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("overlaps referenced file data"));
        assert_eq!(std::fs::read(binary.path()).unwrap(), before);
    }

    #[test]
    fn test_macho_zero_length_linkedit_range_does_not_constrain_signature() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, false);
        add_macho_linkedit_command_and_signature(binary.path(), macho::LC_DATA_IN_CODE, 0x300, 0);

        assert!(read_from_path(binary.path()).unwrap().is_none());
    }

    #[test]
    fn test_macho_template_requires_authenticated_reader_capability() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, false);
        let error = validate_runtime_template_reader(binary.path()).unwrap_err();
        assert!(error.to_string().contains("capability record is invalid"));

        add_reader_capability(binary.path(), 0x240, MACHO_READER_LAYOUT_KIND);
        validate_runtime_template_reader(binary.path()).unwrap();
    }

    #[test]
    fn test_macho_marker_in_ordinary_read_only_section_cannot_claim_reader_capability() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, false);
        add_reader_capability(binary.path(), 0x200, MACHO_READER_LAYOUT_KIND);

        let error = validate_runtime_template_reader(binary.path()).unwrap_err();
        assert!(error.to_string().contains("capability record is invalid"));
    }

    #[test]
    fn test_macho_signature_bytes_cannot_claim_reader_capability() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        add_reader_capability(binary.path(), 0x310, MACHO_READER_LAYOUT_KIND);

        let error = validate_runtime_template_reader(binary.path()).unwrap_err();
        assert!(error.to_string().contains("capability record is invalid"));
    }

    #[test]
    fn test_pe32_and_pe64_use_authenticated_anchor_section() {
        for is_64 in [false, true] {
            let binary = pe_fixture(is_64);
            let header = RuntimeDataHeader::for_name(if is_64 { "pe64" } else { "pe32" });
            append_to_binary(binary.path(), &header, None).unwrap();

            let file = File::open(binary.path()).unwrap();
            let file_len = file.metadata().unwrap().len();
            let layout = read_pe_layout(&file, file_len).unwrap();
            let anchor = layout.anchor.unwrap();
            assert_eq!(anchor.raw_end, layout.max_raw_end);
            assert_eq!(layout.certificate.map(|item| item.offset), None);
            assert_eq!(file_len % PE_CERTIFICATE_ALIGNMENT, 0);
            assert_eq!(
                read_from_path(binary.path()).unwrap().unwrap().header,
                header
            );
        }
    }

    #[test]
    fn test_pe_template_requires_authenticated_reader_capability() {
        let binary = pe_fixture(true);
        let error = validate_runtime_template_reader(binary.path()).unwrap_err();
        assert!(error.to_string().contains("does not declare"));

        add_pe_reader_capability_section(binary.path());
        validate_runtime_template_reader(binary.path()).unwrap();
    }

    #[test]
    fn test_pe_marker_in_ordinary_read_only_section_cannot_claim_reader_capability() {
        let binary = pe_fixture(true);
        add_reader_capability(binary.path(), 0x210, PE_READER_LAYOUT_KIND);

        let error = validate_runtime_template_reader(binary.path()).unwrap_err();
        assert!(error.to_string().contains("does not declare"));
    }

    #[test]
    fn test_pe_header_bytes_cannot_claim_reader_capability() {
        let binary = pe_fixture(true);
        add_reader_capability(binary.path(), 0x40, PE_READER_LAYOUT_KIND);

        let error = validate_runtime_template_reader(binary.path()).unwrap_err();
        assert!(error.to_string().contains("does not declare"));
    }

    #[test]
    fn test_pe_writer_rejects_platform_architecture_mismatch() {
        let binary = pe_fixture(true);
        let before = std::fs::read(binary.path()).unwrap();
        let mut header = RuntimeDataHeader::for_name("wrong-architecture");
        header.platform = "win-arm64".to_string();

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(
            error.to_string().contains("does not match platform"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), before);
    }

    #[test]
    fn test_pe_writer_rejects_platform_image_kind_mismatch() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        write_u16(
            &mut bytes,
            pe_offset + 4,
            object::pe::IMAGE_FILE_MACHINE_I386.0,
        );
        std::fs::write(binary.path(), &bytes).unwrap();
        let mut header = RuntimeDataHeader::for_name("wrong-image-kind");
        header.platform = "win-32".to_string();

        let error = append_to_binary(binary.path(), &header, None).unwrap_err();

        assert!(error.to_string().contains("image kind PE32+"), "{error}");
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_pe_writer_rejects_non_executable_template() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        write_u16(&mut bytes, pe_offset + 22, pe::IMAGE_FILE_DLL.0);
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("not-executable"),
            None,
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("not an executable image"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_pe_writer_rejects_loader_incompatible_alignment() {
        for (section_alignment, file_alignment) in [(0x100, 0x100), (0x100, 0x200)] {
            let binary = pe_fixture(true);
            let mut bytes = std::fs::read(binary.path()).unwrap();
            let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
            let optional = pe_offset + 24;
            write_u32(&mut bytes, optional + 32, section_alignment);
            write_u32(&mut bytes, optional + 36, file_alignment);
            std::fs::write(binary.path(), &bytes).unwrap();

            let error = append_to_binary(
                binary.path(),
                &RuntimeDataHeader::for_name("bad-alignment"),
                None,
            )
            .unwrap_err();

            assert!(
                error.to_string().contains("alignment is invalid"),
                "{error}"
            );
            assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
        }
    }

    #[test]
    fn test_pe_writer_accepts_spec_compliant_low_alignment() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        let section = pe_section_table_offset(&bytes);
        write_u32(&mut bytes, optional + 32, 0x200);
        write_u32(&mut bytes, optional + 36, 0x200);
        write_u32(&mut bytes, optional + 56, 0x400);
        write_u32(&mut bytes, section + 12, 0x200);
        std::fs::write(binary.path(), &bytes).unwrap();

        append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("low-alignment"),
            None,
        )
        .unwrap();

        let file = File::open(binary.path()).unwrap();
        let layout = read_pe_layout(&file, file.metadata().unwrap().len()).unwrap();
        let anchor = layout.anchor.unwrap();
        assert_eq!(anchor.offset, 0x400);
        assert_eq!(layout.max_virtual_end, 0x600);
    }

    #[test]
    fn test_low_alignment_pe_requires_matching_file_and_virtual_offsets() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        bytes.resize(0x600, 0);
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        let section = pe_section_table_offset(&bytes);
        write_u32(&mut bytes, optional + 32, 0x200);
        write_u32(&mut bytes, optional + 36, 0x200);
        write_u32(&mut bytes, optional + 56, 0x400);
        write_u32(&mut bytes, section + 12, 0x200);
        write_u32(&mut bytes, section + 20, 0x400);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("file offset does not match its virtual address"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_writer_preserves_oversized_aligned_size_of_image() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        write_u32(&mut bytes, optional + 56, 0x4000);
        std::fs::write(binary.path(), bytes).unwrap();

        append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("reserved-image"),
            None,
        )
        .unwrap();

        let bytes = std::fs::read(binary.path()).unwrap();
        assert_eq!(
            u32::from_le_bytes(bytes[optional + 56..optional + 60].try_into().unwrap()),
            0x4000
        );
    }

    #[test]
    fn test_pe_writer_rejects_image_that_would_exceed_loader_limit() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        let section = pe_section_table_offset(&bytes);
        write_u32(&mut bytes, optional + 32, 0x4000_0000);
        write_u32(&mut bytes, optional + 56, 0x8000_0000);
        write_u32(&mut bytes, section + 12, 0x4000_0000);
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("oversized-image"),
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("2 GiB loader limit"), "{error}");
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_pe_writer_accepts_maximum_header_size() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        let section = pe_section_table_offset(&bytes);
        let section_data = bytes[0x200..0x400].to_vec();
        bytes[0x200..0x400].fill(0);
        let header_size = usize::try_from(MAX_PE_HEADER_BYTES).unwrap();
        bytes.resize(header_size + 0x200, 0);
        bytes[header_size..header_size + 0x200].copy_from_slice(&section_data);
        write_u32(
            &mut bytes,
            optional + 60,
            u32::try_from(MAX_PE_HEADER_BYTES).unwrap(),
        );
        write_u32(
            &mut bytes,
            optional + 56,
            u32::try_from(MAX_PE_HEADER_BYTES + 0x1000).unwrap(),
        );
        write_u32(
            &mut bytes,
            section + 12,
            u32::try_from(MAX_PE_HEADER_BYTES).unwrap(),
        );
        write_u32(
            &mut bytes,
            section + 20,
            u32::try_from(MAX_PE_HEADER_BYTES).unwrap(),
        );
        std::fs::write(binary.path(), bytes).unwrap();

        let header = RuntimeDataHeader::for_name("maximum-headers");
        append_to_binary(binary.path(), &header, None).unwrap();

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header, header);
    }

    #[test]
    fn test_pe_writer_rejects_oversized_headers_without_modifying_input() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        write_u32(
            &mut bytes,
            optional + 60,
            u32::try_from(MAX_PE_HEADER_BYTES + 0x200).unwrap(),
        );
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("oversized-headers"),
            None,
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("SizeOfHeaders exceeds"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_pe_writer_rejects_unaligned_image_size_fields() {
        for (field, value, expected) in [
            (60, 0x300, "SizeOfHeaders is not file-aligned"),
            (56, 0x2100, "SizeOfImage is not section-aligned"),
        ] {
            let binary = pe_fixture(true);
            let mut bytes = std::fs::read(binary.path()).unwrap();
            let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
            write_u32(&mut bytes, pe_offset + 24 + field, value);
            std::fs::write(binary.path(), &bytes).unwrap();

            let error = append_to_binary(
                binary.path(),
                &RuntimeDataHeader::for_name("bad-size"),
                None,
            )
            .unwrap_err();

            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
        }
    }

    #[test]
    fn test_pe_runtime_data_must_fit_certificate_offset() {
        let error = pe_runtime_content_layout(u64::from(u32::MAX) - 7, 16).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("too large for a certificate table offset"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_reader_accepts_zero_alignment_before_overlay_content() {
        let binary = pe_fixture(true);
        let mut header = RuntimeDataHeader::for_name("aligned");
        while serde_json::to_vec(&header)
            .unwrap()
            .len()
            .is_multiple_of(PE_CERTIFICATE_ALIGNMENT as usize)
        {
            header.runtime_lock.push('x');
        }
        append_to_binary(binary.path(), &header, None).unwrap();
        let mut file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_pe_layout(&file, file_len).unwrap();
        let anchor = layout.anchor.unwrap();
        file.seek(SeekFrom::Start(anchor.offset)).unwrap();
        let mut footer = [0_u8; FOOTER_LEN];
        file.read_exact(&mut footer).unwrap();
        let decoded = decode_footer(&footer).unwrap().unwrap();
        let (payload_start, payload_end) =
            pe_runtime_content_layout(anchor.raw_end, decoded.header_len).unwrap();
        assert!(payload_start > anchor.raw_end);
        assert_eq!(payload_end, file_len);
        ensure_zero_file_range(
            &mut file,
            anchor.raw_end,
            payload_start - anchor.raw_end,
            "alignment",
        )
        .unwrap();
        drop(file);
        add_pe_certificate(binary.path(), b"certificate");

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header, header);
    }

    #[test]
    fn test_pe_reader_rejects_nonzero_alignment_before_overlay_content() {
        let binary = pe_fixture(false);
        let mut header = RuntimeDataHeader::for_name("nonzero-alignment");
        while serde_json::to_vec(&header)
            .unwrap()
            .len()
            .is_multiple_of(PE_CERTIFICATE_ALIGNMENT as usize)
        {
            header.runtime_lock.push('x');
        }
        append_to_binary(binary.path(), &header, None).unwrap();
        let file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let layout = read_pe_layout(&file, file_len).unwrap();
        let anchor = layout.anchor.unwrap();
        drop(file);
        let mut file = OpenOptions::new().write(true).open(binary.path()).unwrap();
        file.seek(SeekFrom::Start(anchor.raw_end)).unwrap();
        file.write_all(&[1]).unwrap();
        drop(file);
        add_pe_certificate(binary.path(), b"certificate");

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("alignment padding is not zero"));
    }

    #[test]
    fn test_unsigned_legacy_pe_overlay_remains_readable() {
        let binary = pe_fixture(true);
        let header = RuntimeDataHeader::for_name("legacy");
        let payload = runtime_payload(&header);
        OpenOptions::new()
            .append(true)
            .open(binary.path())
            .unwrap()
            .write_all(&payload)
            .unwrap();

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header, header);
    }

    #[test]
    fn test_signed_legacy_pe_overlay_is_not_trusted() {
        let binary = pe_fixture(true);
        let header = RuntimeDataHeader::for_name("legacy");
        let payload = runtime_payload(&header);
        let mut file = OpenOptions::new().append(true).open(binary.path()).unwrap();
        file.write_all(&payload).unwrap();
        let footer_end = file.metadata().unwrap().len();
        let certificate_offset = align_up(footer_end, PE_CERTIFICATE_ALIGNMENT).unwrap();
        file.write_all(&vec![0; (certificate_offset - footer_end) as usize])
            .unwrap();
        drop(file);
        add_pe_certificate(binary.path(), b"certificate");

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no authenticated anchor section")
        );
    }

    #[test]
    fn test_pe_writer_rejects_existing_overlay_without_modifying_input() {
        let binary = pe_fixture(true);
        OpenOptions::new()
            .append(true)
            .open(binary.path())
            .unwrap()
            .write_all(b"existing overlay")
            .unwrap();
        let before = std::fs::read(binary.path()).unwrap();

        let error = append_to_binary(binary.path(), &RuntimeDataHeader::for_name("overlay"), None)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("data outside its declared sections"),
            "{error}"
        );
        assert_eq!(std::fs::read(binary.path()).unwrap(), before);
    }

    #[test]
    fn test_pe_writer_rejects_signed_template_without_modifying_input() {
        let binary = pe_fixture(true);
        add_pe_certificate(binary.path(), b"certificate");
        let before = std::fs::read(binary.path()).unwrap();

        let error = append_to_binary(binary.path(), &RuntimeDataHeader::for_name("signed"), None)
            .unwrap_err();

        assert!(error.to_string().contains("must be unsigned"), "{error}");
        assert_eq!(std::fs::read(binary.path()).unwrap(), before);
    }

    #[test]
    fn test_pe_writer_rejects_nonzero_section_header_slack() {
        let binary = pe_fixture(false);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let next_section = pe_section_table_offset(&bytes) + pe::IMAGE_SIZEOF_SECTION_HEADER;
        bytes[next_section] = 1;
        std::fs::write(binary.path(), &bytes).unwrap();

        let error = append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("header-slack"),
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("not zero-filled"), "{error}");
        assert_eq!(std::fs::read(binary.path()).unwrap(), bytes);
    }

    #[test]
    fn test_pe_overlapping_raw_sections_are_rejected() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        write_u16(&mut bytes, pe_offset + 6, 2);
        let section = pe_section_table_offset(&bytes);
        bytes.copy_within(section..section + 40, section + 40);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("raw sections overlap"));
    }

    #[test]
    fn test_pe_section_table_must_be_ordered_by_virtual_address() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        bytes.resize(0x600, 0xa5);
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        write_u16(&mut bytes, pe_offset + 6, 2);
        write_u32(&mut bytes, optional + 56, 0x3000);
        let section = pe_section_table_offset(&bytes);
        let mut later = bytes[section..section + 40].to_vec();
        later[..8].fill(0);
        later[..5].copy_from_slice(b".data");
        write_u32(&mut later, 12, 0x2000);
        write_u32(&mut later, 20, 0x400);
        let earlier = bytes[section..section + 40].to_vec();
        bytes[section..section + 40].copy_from_slice(&later);
        bytes[section + 40..section + 80].copy_from_slice(&earlier);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error.to_string().contains("not ordered by virtual address"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_virtual_sections_must_be_adjacent() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        bytes.resize(0x600, 0xa5);
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        write_u16(&mut bytes, pe_offset + 6, 2);
        write_u32(&mut bytes, optional + 56, 0x4000);
        let section = pe_section_table_offset(&bytes);
        let mut later = bytes[section..section + 40].to_vec();
        later[..8].fill(0);
        later[..5].copy_from_slice(b".data");
        write_u32(&mut later, 12, 0x3000);
        write_u32(&mut later, 20, 0x400);
        bytes[section + 40..section + 80].copy_from_slice(&later);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("not adjacent"), "{error}");
    }

    #[test]
    fn test_pe_raw_sections_must_follow_virtual_address_order() {
        let binary = pe_fixture(true);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        bytes.resize(0x600, 0xa5);
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional = pe_offset + 24;
        write_u16(&mut bytes, pe_offset + 6, 2);
        write_u32(&mut bytes, optional + 56, 0x3000);
        let section = pe_section_table_offset(&bytes);
        write_u32(&mut bytes, section + 20, 0x400);
        let mut later = bytes[section..section + 40].to_vec();
        later[..8].fill(0);
        later[..5].copy_from_slice(b".data");
        write_u32(&mut later, 12, 0x2000);
        write_u32(&mut later, 20, 0x200);
        bytes[section + 40..section + 80].copy_from_slice(&later);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error.to_string().contains("not ordered by virtual address"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_section_overlapping_headers_is_rejected() {
        let binary = pe_fixture(false);
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let section = pe_section_table_offset(&bytes);
        write_u32(&mut bytes, section + 20, 0x100);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("section overlaps its headers"));
    }

    #[test]
    fn test_pe_reader_ignores_valid_shadow_stamp_inside_certificate_table() {
        for is_64 in [false, true] {
            let binary = pe_fixture(is_64);
            let legitimate = RuntimeDataHeader::for_name("legitimate");
            append_to_binary(binary.path(), &legitimate, None).unwrap();
            let forged = runtime_payload(&RuntimeDataHeader::for_name("forged"));
            add_pe_certificate(binary.path(), &forged);

            let data = read_from_path(binary.path()).unwrap().unwrap();
            assert_eq!(data.header.runtime_name, "legitimate");
        }
    }

    #[test]
    fn test_pe_reader_rejects_malformed_certificate_entry() {
        let binary = pe_fixture(true);
        append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("legitimate"),
            None,
        )
        .unwrap();
        let certificate_offset = add_pe_certificate(binary.path(), b"certificate");
        let mut bytes = std::fs::read(binary.path()).unwrap();
        write_u32(&mut bytes, certificate_offset as usize, 7);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("entry is too small"), "{error}");
    }

    #[test]
    fn test_pe_reader_accepts_certificate_entry_limit() {
        let binary = pe_fixture(true);
        let header = RuntimeDataHeader::for_name("certificate-limit");
        append_to_binary(binary.path(), &header, None).unwrap();
        add_pe_certificate_entries(binary.path(), MAX_PE_CERTIFICATE_ENTRIES);

        let data = read_from_path(binary.path()).unwrap().unwrap();
        assert_eq!(data.header, header);
    }

    #[test]
    fn test_pe_reader_rejects_excessive_certificate_entries() {
        let binary = pe_fixture(true);
        append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("excessive-certificates"),
            None,
        )
        .unwrap();
        add_pe_certificate_entries(binary.path(), MAX_PE_CERTIFICATE_ENTRIES + 1);

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error.to_string().contains("certificate table exceeds"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_reader_rejects_certificate_moved_past_authenticated_payload() {
        let binary = pe_fixture(true);
        append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("legitimate"),
            None,
        )
        .unwrap();
        let certificate_offset = add_pe_certificate(binary.path(), b"certificate") as usize;
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let security_offset = pe_security_directory_offset(&bytes);
        bytes.splice(certificate_offset..certificate_offset, [0_u8; 8]);
        write_u32(
            &mut bytes,
            security_offset,
            u32::try_from(certificate_offset + 8).unwrap(),
        );
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not end at the certificate table offset"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_reader_rejects_corrupt_anchor_instead_of_shadow_stamp() {
        let binary = pe_fixture(true);
        let legitimate = RuntimeDataHeader::for_name("legitimate");
        append_to_binary(binary.path(), &legitimate, None).unwrap();
        let forged = runtime_payload(&RuntimeDataHeader::for_name("forged"));
        let certificate_offset = add_pe_certificate(binary.path(), &forged);

        let mut file = OpenOptions::new().write(true).open(binary.path()).unwrap();
        file.seek(SeekFrom::Start(certificate_offset - FOOTER_LEN as u64 - 1))
            .unwrap();
        file.write_all(b"!").unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("header checksum mismatch"));
    }

    #[test]
    fn test_pe_reader_rejects_modified_authenticated_anchor() {
        let binary = pe_fixture(true);
        let header = RuntimeDataHeader::for_name("legitimate");
        append_to_binary(binary.path(), &header, None).unwrap();
        let file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let anchor = read_pe_layout(&file, file_len).unwrap().anchor.unwrap();
        drop(file);

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(binary.path())
            .unwrap();
        file.seek(SeekFrom::Start(anchor.offset + 16)).unwrap();
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).unwrap();
        file.seek(SeekFrom::Start(anchor.offset + 16)).unwrap();
        file.write_all(&[byte[0] ^ 1]).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(error.to_string().contains("header checksum mismatch"));
    }

    #[test]
    fn test_pe_reader_rejects_nonzero_authenticated_anchor_padding() {
        let binary = pe_fixture(true);
        append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("legitimate"),
            None,
        )
        .unwrap();
        let file = File::open(binary.path()).unwrap();
        let file_len = file.metadata().unwrap().len();
        let anchor = read_pe_layout(&file, file_len).unwrap().anchor.unwrap();
        drop(file);

        let mut file = OpenOptions::new().write(true).open(binary.path()).unwrap();
        file.seek(SeekFrom::Start(anchor.offset + FOOTER_LEN as u64))
            .unwrap();
        file.write_all(&[1]).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error.to_string().contains("section padding is not zero"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_reader_rejects_duplicate_anchor_section() {
        let binary = pe_fixture(true);
        append_to_binary(
            binary.path(),
            &RuntimeDataHeader::for_name("legitimate"),
            None,
        )
        .unwrap();
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let section_table = pe_section_table_offset(&bytes);
        let section_count =
            u16::from_le_bytes(bytes[pe_offset + 6..pe_offset + 8].try_into().unwrap());
        let anchor_header = section_table + (usize::from(section_count) - 1) * 40;
        let duplicate_header = anchor_header + 40;
        let anchor_bytes: [u8; 40] = bytes[anchor_header..anchor_header + 40].try_into().unwrap();
        bytes[duplicate_header..duplicate_header + 40].copy_from_slice(&anchor_bytes);
        write_u16(&mut bytes, pe_offset + 6, section_count + 1);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("multiple conda-ship anchor sections"),
            "{error}"
        );
    }

    #[test]
    fn test_pe_reader_rejects_incomplete_security_directory() {
        let binary = pe_fixture(true);
        let header = RuntimeDataHeader::for_name("legitimate");
        append_to_binary(binary.path(), &header, None).unwrap();
        let mut bytes = std::fs::read(binary.path()).unwrap();
        let security_offset = pe_security_directory_offset(&bytes);
        let file_len = bytes.len() as u32;
        write_u32(&mut bytes, security_offset, file_len);
        std::fs::write(binary.path(), bytes).unwrap();

        let error = read_from_path(binary.path()).unwrap_err();
        assert!(
            error.to_string().contains("incomplete certificate range"),
            "{error}"
        );
    }

    #[test]
    fn test_append_failure_leaves_macho_template_byte_identical() {
        let binary = macho_fixture(macho::CPU_TYPE_ARM64, true);
        let before = std::fs::read(binary.path()).unwrap();
        let bundle_directory = tempfile::tempdir().unwrap();
        let header = RuntimeDataHeader::for_name("snek");

        let error =
            append_to_binary(binary.path(), &header, Some(bundle_directory.path())).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert_eq!(std::fs::read(binary.path()).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn test_append_rejects_symbolic_link_input() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let link = directory.path().join("link");
        std::fs::write(&target, b"binary").unwrap();
        symlink(&target, &link).unwrap();

        let error =
            append_to_binary(&link, &RuntimeDataHeader::for_name("snek"), None).unwrap_err();

        assert!(error.to_string().contains("symbolic link"));
        assert_eq!(std::fs::read(&target).unwrap(), b"binary");
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    }

    #[test]
    fn test_append_and_read_runtime_data_with_bundle() {
        let binary = tempfile::NamedTempFile::new().unwrap();
        let bundle = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(binary.path(), b"binary").unwrap();
        std::fs::write(bundle.path(), b"bundle data").unwrap();

        let header = RuntimeDataHeader::for_name("snek");
        append_to_binary(binary.path(), &header, Some(bundle.path())).unwrap();

        let data = read_from_path(binary.path()).unwrap().unwrap();
        let embedded = data.bundle.unwrap();
        embedded.verify().unwrap();
        let mut contents = String::new();
        embedded
            .open_verified()
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        assert_eq!(embedded.len(), "bundle data".len() as u64);
        assert_eq!(contents, "bundle data");
    }

    #[test]
    fn test_verified_bundle_snapshot_survives_executable_path_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("runtime");
        let replaced = directory.path().join("runtime.replaced");
        let bundle = directory.path().join("bundle.tar.zst");
        std::fs::write(&binary, b"binary").unwrap();
        std::fs::write(&bundle, b"verified bundle data").unwrap();

        append_to_binary(&binary, &RuntimeDataHeader::for_name("snek"), Some(&bundle)).unwrap();
        let embedded = read_from_path(&binary).unwrap().unwrap().bundle.unwrap();
        let mut verified = embedded.open_verified().unwrap();
        std::fs::rename(&binary, replaced).unwrap();
        std::fs::write(&binary, b"attacker-controlled replacement").unwrap();

        let mut contents = String::new();
        verified.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "verified bundle data");
    }

    #[test]
    fn test_corrupt_runtime_data_is_rejected() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"binary").unwrap();

        let header = RuntimeDataHeader::for_name("snek");
        append_to_binary(tmp.path(), &header, None).unwrap();

        let mut file = OpenOptions::new().write(true).open(tmp.path()).unwrap();
        file.seek(SeekFrom::End(-(FOOTER_LEN as i64) - 1)).unwrap();
        file.write_all(b"!").unwrap();

        let err = read_from_path(tmp.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("header checksum mismatch"));
    }

    #[test]
    fn test_corrupt_bundle_is_rejected_when_verified() {
        let binary = tempfile::NamedTempFile::new().unwrap();
        let bundle = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(binary.path(), b"binary").unwrap();
        std::fs::write(bundle.path(), b"bundle data").unwrap();

        let header = RuntimeDataHeader::for_name("snek");
        append_to_binary(binary.path(), &header, Some(bundle.path())).unwrap();

        let data = read_from_path(binary.path()).unwrap().unwrap();
        let embedded = data.bundle.unwrap();

        let mut file = OpenOptions::new().write(true).open(binary.path()).unwrap();
        file.seek(SeekFrom::Start(embedded.offset)).unwrap();
        file.write_all(b"!").unwrap();

        let err = embedded.verify().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("bundle checksum mismatch"));
    }
}
