use crate::Tin;
use crate::bir_xml::{decode_bir_value, parse_bir_xml_encoded_occurrences_checked};
use crate::db::{Database, DbError, LegacyFiledReturn};
use crate::official_import::find_capability_identity;
use crate::profile::{TaxProfileVersionStatus, TaxpayerProfile};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

const FINAL_COPY_TRAILERS: [&str; 2] = [
    "All Rights Reserved BIR 2012.0",
    "All Rights Reserved BIR 2014.0",
];

#[derive(Debug, Error)]
pub enum LegacyMigrationError {
    #[error("not an eBIRForms folder: {0}")]
    NotALegacyFolder(PathBuf),
    #[error("invalid legacy TIN: {0}")]
    InvalidTin(String),
    #[error("could not read legacy folder: {0}")]
    Io(#[from] std::io::Error),
    #[error("database migration failed: {0}")]
    Database(#[from] DbError),
    #[error("could not serialize migrated data: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct LegacyProfile {
    pub source_path: PathBuf,
    pub tin: String,
    pub branch: String,
    pub full_name: String,
    pub rdo_code: String,
    pub line_of_business: String,
    pub registered_address: String,
    pub zip_code: String,
    pub phone: String,
    pub email: String,
    pub default_form_type: String,
}

#[derive(Debug, Clone)]
pub struct LegacyReturn {
    pub source_path: PathBuf,
    pub raw_file: Vec<u8>,
    pub sha256: String,
    pub tin: String,
    pub branch: String,
    pub form_identity: String,
    pub form_id: Option<String>,
    pub period: String,
    pub amendment_version: Option<u32>,
    pub email_copy: Option<String>,
    pub encrypted: bool,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyScanIssue {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct LegacyScan {
    pub profiles: Vec<LegacyProfile>,
    pub returns: Vec<LegacyReturn>,
    pub issues: Vec<LegacyScanIssue>,
    pub savefile_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct LegacyMigrationReport {
    pub profiles_created: usize,
    pub profiles_updated: usize,
    pub profiles_unchanged: usize,
    pub profile_fields_filled: usize,
    pub returns_imported: usize,
    pub returns_already_present: usize,
    pub issues: Vec<LegacyScanIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProfileKey {
    tin: String,
    branch: u32,
}

struct ParsedReturnFilename {
    tin: String,
    branch: String,
    form_identity: String,
    period: String,
    amendment_version: Option<u32>,
    email_copy: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ProfileValues {
    full_name: String,
    rdo_code: String,
    line_of_business: String,
    registered_address: String,
    zip_code: String,
    phone: String,
    email: String,
    default_form_type: String,
}

pub fn discover_legacy_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let system_drive = std::env::var_os("SystemDrive")
            .unwrap_or_else(|| "C:".into())
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string();
        let mut candidates = vec![
            PathBuf::from(format!("{system_drive}\\eBIRForms")),
            PathBuf::from(r"C:\eBIRForms"),
        ];
        candidates.retain(|path| path.is_dir());
        let mut roots = Vec::new();
        for candidate in candidates {
            if !roots.iter().any(|existing: &PathBuf| {
                existing
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&candidate.to_string_lossy())
            }) {
                roots.push(candidate);
            }
        }
        roots
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

pub fn scan_legacy_folder(path: &Path) -> Result<LegacyScan, LegacyMigrationError> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| LegacyMigrationError::NotALegacyFolder(path.to_path_buf()))?;
    if !metadata.is_dir() {
        return Err(LegacyMigrationError::NotALegacyFolder(path.to_path_buf()));
    }

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let (profile_dirs, return_dirs, savefile_dirs) = if name.eq_ignore_ascii_case("profile") {
        (vec![path.to_path_buf()], Vec::new(), Vec::new())
    } else if name.eq_ignore_ascii_case("IAF_RDO_Copy") {
        (Vec::new(), vec![path.to_path_buf()], Vec::new())
    } else {
        let mut profile_dirs = Vec::new();
        let mut return_dirs = Vec::new();
        let mut savefile_dirs = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let child = entry.file_name();
            let child = child.to_string_lossy();
            if child.eq_ignore_ascii_case("profile") {
                profile_dirs.push(entry.path());
            } else if child.eq_ignore_ascii_case("IAF_RDO_Copy") {
                return_dirs.push(entry.path());
            } else if child.eq_ignore_ascii_case("savefile") {
                savefile_dirs.push(entry.path());
            }
        }
        if profile_dirs.is_empty() && return_dirs.is_empty() {
            return Err(LegacyMigrationError::NotALegacyFolder(path.to_path_buf()));
        }
        (profile_dirs, return_dirs, savefile_dirs)
    };

    let mut scan = LegacyScan::default();
    for folder in profile_dirs {
        scan_profiles(&folder, &mut scan)?;
    }
    for folder in return_dirs {
        scan_returns(&folder, &mut scan)?;
    }
    for folder in savefile_dirs {
        scan.savefile_count += count_files(&folder)?;
    }
    scan.profiles
        .sort_by(|a, b| a.source_path.cmp(&b.source_path));
    scan.returns
        .sort_by(|a, b| a.source_path.cmp(&b.source_path));
    scan.issues.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(scan)
}

fn count_files(folder: &Path) -> Result<usize, LegacyMigrationError> {
    Ok(std::fs::read_dir(folder)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .count())
}

fn scan_profiles(folder: &Path, scan: &mut LegacyScan) -> Result<(), LegacyMigrationError> {
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if !has_xml_extension(&path) {
            scan.issues.push(issue(&path, "not an XML file"));
            continue;
        }
        let Some((tin, branch)) = parse_profile_filename(&path) else {
            scan.issues.push(issue(&path, "invalid profile filename"));
            continue;
        };
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) => {
                scan.issues
                    .push(issue(&path, &format!("could not read profile: {error}")));
                continue;
            }
        };
        match parse_profile_contents(&contents, &path, tin, branch) {
            Ok(profile) => scan.profiles.push(profile),
            Err(reason) => scan.issues.push(issue(&path, &reason)),
        }
    }
    Ok(())
}

fn scan_returns(folder: &Path, scan: &mut LegacyScan) -> Result<(), LegacyMigrationError> {
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if !has_xml_extension(&path) {
            scan.issues.push(issue(&path, "not an XML file"));
            continue;
        }
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            scan.issues.push(issue(&path, "invalid return filename"));
            continue;
        };
        let Some(parsed_filename) = parse_return_filename(filename) else {
            scan.issues.push(issue(&path, "invalid return filename"));
            continue;
        };
        let ParsedReturnFilename {
            tin,
            branch,
            form_identity,
            period,
            amendment_version,
            email_copy,
        } = parsed_filename;
        let raw_file = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                scan.issues
                    .push(issue(&path, &format!("could not read return: {error}")));
                continue;
            }
        };
        let encrypted = !raw_file
            .windows(b"</div>".len())
            .any(|window| window == b"</div>");
        // Encrypted IAF copies need the IAF key, which this build does not
        // carry; only plaintext savefiles are read.
        if encrypted {
            scan.issues
                .push(issue(&path, "encrypted return (IAF copy) cannot be read"));
            continue;
        }
        let plaintext = raw_file.clone();
        let Ok(contents) = std::str::from_utf8(&plaintext) else {
            scan.issues
                .push(issue(&path, "return contents are not UTF-8"));
            continue;
        };
        if !FINAL_COPY_TRAILERS
            .iter()
            .any(|trailer| contents.contains(trailer))
        {
            scan.issues
                .push(issue(&path, "final copy trailer is missing"));
            continue;
        }
        let fields = match parse_bir_xml_encoded_occurrences_checked(contents) {
            Ok(occurrences) => occurrences
                .into_iter()
                // Official exports are not guaranteed to use one codec; keep the
                // raw body when neither decoder applies rather than dropping it.
                .map(|field| {
                    let value = decode_bir_value(&field.encoded_value)
                        .unwrap_or_else(|| field.encoded_value.clone());
                    (field.field_id, value)
                })
                .collect(),
            Err(error) => {
                scan.issues
                    .push(issue(&path, &format!("invalid return XML: {error}")));
                continue;
            }
        };
        let form_id =
            find_capability_identity(&form_identity).map(|capability| capability.form_id.into());
        scan.returns.push(LegacyReturn {
            source_path: path,
            sha256: hex::encode(Sha256::digest(&raw_file)),
            raw_file,
            tin,
            branch,
            form_identity,
            form_id,
            period,
            amendment_version,
            email_copy,
            encrypted,
            fields,
        });
    }
    Ok(())
}

fn issue(path: &Path, reason: &str) -> LegacyScanIssue {
    LegacyScanIssue {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

fn has_xml_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
}

fn parse_profile_filename(path: &Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_str()?;
    if !stem.bytes().all(|byte| byte.is_ascii_digit()) || !(12..=14).contains(&stem.len()) {
        return None;
    }
    Some((stem[..9].to_string(), stem[9..].to_string()))
}

fn parse_profile_contents(
    contents: &str,
    source_path: &Path,
    tin: String,
    branch: String,
) -> Result<LegacyProfile, String> {
    // Tag-oriented: official profile files put the XML declaration and every
    // div on a single line.
    let mut fields = BTreeMap::new();
    let mut rest = contents;
    while let Some(start) = rest.find("<div>") {
        rest = &rest[start + "<div>".len()..];
        let Some(end) = rest.find("</div>") else {
            break;
        };
        let div_contents = &rest[..end];
        rest = &rest[end..];
        let Some((key, value)) = div_contents.split_once('=') else {
            continue;
        };
        let terminator = format!("{key}=");
        let Some(value) = value.strip_suffix(&terminator) else {
            continue;
        };
        if !key.is_empty() {
            fields.insert(key.to_string(), value.to_string());
        }
    }
    if tin.bytes().all(|byte| byte == b'0') {
        return Err("placeholder profile: TIN is all zeros".to_string());
    }
    // The official app writes the literal "undefined" for fields its JS never set.
    for value in fields.values_mut() {
        if value == "undefined" {
            value.clear();
        }
    }
    let read = |key: &str| fields.get(key).cloned().unwrap_or_default();
    let mut full_name = read("fn");
    let mut line_of_business = read("lob");
    let mut registered_address = read("regAddr");
    full_name = js_unescape(&full_name);
    line_of_business = js_unescape(&line_of_business);
    registered_address = js_unescape(&registered_address);
    if !fields.contains_key("fn") {
        return Err("profile field fn is missing".to_string());
    }

    Ok(LegacyProfile {
        source_path: source_path.to_path_buf(),
        tin,
        branch,
        full_name,
        rdo_code: read("rdo"),
        line_of_business,
        registered_address,
        zip_code: read("zip"),
        phone: read("telNo"),
        email: read("email"),
        default_form_type: read("formType"),
    })
}

fn js_unescape(value: &str) -> String {
    let mut utf16 = Vec::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%' {
            let mut peeked = chars.clone();
            if peeked
                .next()
                .is_some_and(|character| character == 'u' || character == 'U')
            {
                let _ = chars.next();
                let digits = chars.clone().take(4).collect::<String>();
                if digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    for _ in 0..4 {
                        let _ = chars.next();
                    }
                    if let Ok(code_unit) = u16::from_str_radix(&digits, 16) {
                        utf16.push(code_unit);
                        continue;
                    }
                } else {
                    utf16.extend("%u".encode_utf16());
                    continue;
                }
            } else {
                let digits = chars.clone().take(2).collect::<String>();
                if digits.len() == 2 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    let _ = chars.next();
                    let _ = chars.next();
                    if let Ok(code_unit) = u16::from_str_radix(&digits, 16) {
                        utf16.push(code_unit);
                        continue;
                    }
                }
            }
        }
        let mut encoded = [0; 2];
        utf16.extend(character.encode_utf16(&mut encoded).iter().copied());
    }
    String::from_utf16_lossy(&utf16)
}

fn parse_return_filename(filename: &str) -> Option<ParsedReturnFilename> {
    let extension_start = filename.len().checked_sub(4)?;
    if !filename
        .get(extension_start..)?
        .eq_ignore_ascii_case(".xml")
    {
        return None;
    }
    let stem = filename.get(..extension_start)?;
    let (stem, email_copy) = if let Some(without_end) = stem.strip_suffix('#') {
        let start = without_end.rfind('#')?;
        (
            &without_end[..start],
            Some(without_end[start + 1..].to_string()),
        )
    } else {
        if stem.contains('#') {
            return None;
        }
        (stem, None)
    };
    let mut parts = stem.split('-');
    let tin_branch = parts.next()?;
    let form_identity = parts.next()?.to_string();
    let named_period = parts.next()?;
    if parts.next().is_some()
        || !tin_branch.bytes().all(|byte| byte.is_ascii_digit())
        || !(12..=14).contains(&tin_branch.len())
        || form_identity.is_empty()
        || !form_identity
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        || named_period.is_empty()
    {
        return None;
    }
    let (period, amendment_version) = strip_amendment_suffix(named_period);
    if period.is_empty() || !period.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ParsedReturnFilename {
        tin: tin_branch[..9].to_string(),
        branch: tin_branch[9..].to_string(),
        form_identity,
        period: period.to_string(),
        amendment_version,
        email_copy,
    })
}

fn strip_amendment_suffix(period: &str) -> (&str, Option<u32>) {
    let Some(index) = period.rfind(['V', 'v']) else {
        return (period, None);
    };
    let version = &period[index + 1..];
    if !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit()) {
        (&period[..index], version.parse().ok())
    } else {
        (period, None)
    }
}

pub fn apply_legacy_migration(
    db: &Database,
    scan: &LegacyScan,
) -> Result<LegacyMigrationReport, LegacyMigrationError> {
    let mut report = LegacyMigrationReport {
        issues: scan.issues.clone(),
        ..LegacyMigrationReport::default()
    };
    let mut existing_profiles = db.list_profiles()?;
    let mut created_profiles = Vec::<TaxpayerProfile>::new();
    let mut updated_keys = BTreeSet::new();
    let mut unchanged_keys = BTreeSet::new();
    let mut modified_existing = BTreeSet::new();
    let mut known_profile_keys = BTreeSet::new();
    let mut profile_file_keys = BTreeSet::new();

    for imported in &scan.profiles {
        let key = profile_key(&imported.tin, &imported.branch)?;
        profile_file_keys.insert(key.clone());
        known_profile_keys.insert(key.clone());
        let values = ProfileValues::from(imported);
        if let Some(profile) = existing_profiles
            .iter_mut()
            .find(|profile| profile_key_from_profile(profile).as_ref() == Some(&key))
        {
            let filled = fill_empty_fields(profile, &values);
            report.profile_fields_filled += filled;
            if filled > 0 {
                modified_existing.insert(profile.tin.full());
                updated_keys.insert(key);
            } else if !updated_keys.contains(&key) {
                unchanged_keys.insert(key);
            }
            continue;
        }
        if let Some(profile) = created_profiles
            .iter_mut()
            .find(|profile| profile_key_from_profile(profile).as_ref() == Some(&key))
        {
            report.profile_fields_filled += fill_empty_fields(profile, &values);
            continue;
        }
        report.profile_fields_filled += values.non_empty_field_count();
        created_profiles.push(new_profile(&imported.tin, &imported.branch, values)?);
    }

    let existing_profile_keys = existing_profiles
        .iter()
        .filter_map(profile_key_from_profile)
        .collect::<BTreeSet<_>>();
    let mut latest_return_by_key = BTreeMap::<ProfileKey, &LegacyReturn>::new();
    for legacy_return in &scan.returns {
        if legacy_return.tin.bytes().all(|byte| byte == b'0') {
            continue;
        }
        let key = profile_key(&legacy_return.tin, &legacy_return.branch)?;
        if profile_file_keys.contains(&key) {
            continue;
        }
        if existing_profile_keys.contains(&key) {
            known_profile_keys.insert(key.clone());
            unchanged_keys.insert(key);
            continue;
        }
        let replace = latest_return_by_key.get(&key).is_none_or(|current| {
            return_period_order(legacy_return) > return_period_order(current)
        });
        if replace {
            latest_return_by_key.insert(key.clone(), legacy_return);
        }
    }
    for (key, legacy_return) in latest_return_by_key {
        if !known_profile_keys.insert(key.clone()) {
            continue;
        }
        let values = values_from_return(legacy_return);
        report.profile_fields_filled += values.non_empty_field_count();
        created_profiles.push(new_profile(
            &legacy_return.tin,
            &legacy_return.branch,
            values,
        )?);
    }

    // Profiles commit separately; re-running the idempotent migration recovers partial failure.
    for profile in &existing_profiles {
        let tin = profile.tin.full();
        if modified_existing.contains(&tin) {
            db.save_profile(profile.clone())?;
        }
    }
    for profile in &created_profiles {
        db.save_profile(profile.clone())?;
    }
    report.profiles_created = created_profiles.len();
    report.profiles_updated = updated_keys.len();
    report.profiles_unchanged = unchanged_keys.difference(&updated_keys).count();

    let transaction = db.conn.unchecked_transaction().map_err(DbError::from)?;
    for legacy_return in &scan.returns {
        let branch = format!(
            "{:05}",
            legacy_return.branch.parse::<u32>().unwrap_or_default()
        );
        let tin = format!("{}{}", legacy_return.tin, branch);
        let record = LegacyFiledReturn {
            id: None,
            sha256: legacy_return.sha256.clone(),
            tin,
            form_identity: legacy_return.form_identity.clone(),
            form_id: legacy_return.form_id.clone(),
            period: legacy_return.period.clone(),
            amendment_version: legacy_return.amendment_version,
            email_copy: legacy_return.email_copy.clone(),
            encrypted: legacy_return.encrypted,
            source_filename: legacy_return
                .source_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            raw_file: legacy_return.raw_file.clone(),
            fields_json: serde_json::to_string(&legacy_return.fields)?,
            imported_at: None,
        };
        if Database::insert_legacy_filed_return_on(&transaction, &record)? {
            report.returns_imported += 1;
        } else {
            report.returns_already_present += 1;
        }
    }
    transaction.commit().map_err(DbError::from)?;

    Ok(report)
}

impl From<&LegacyProfile> for ProfileValues {
    fn from(profile: &LegacyProfile) -> Self {
        Self {
            full_name: profile.full_name.clone(),
            rdo_code: profile.rdo_code.clone(),
            line_of_business: profile.line_of_business.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            phone: profile.phone.clone(),
            email: profile.email.clone(),
            default_form_type: profile.default_form_type.clone(),
        }
    }
}

impl ProfileValues {
    fn non_empty_field_count(&self) -> usize {
        [
            &self.full_name,
            &self.rdo_code,
            &self.line_of_business,
            &self.registered_address,
            &self.zip_code,
            &self.phone,
            &self.email,
            &self.default_form_type,
        ]
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .count()
    }
}

fn values_from_return(legacy_return: &LegacyReturn) -> ProfileValues {
    let get = |suffix| field_suffix(&legacy_return.fields, suffix).unwrap_or_default();
    let full_name = field_suffix(&legacy_return.fields, "txtTaxpayerName")
        .or_else(|| field_suffix(&legacy_return.fields, "taxpayerName"))
        .unwrap_or_default();
    ProfileValues {
        full_name,
        rdo_code: get("txtRDOCode"),
        line_of_business: get("txtLineBus"),
        registered_address: get("txtAddress"),
        zip_code: get("txtZipCode"),
        phone: get("txtTelNum"),
        email: get("txtEmail"),
        default_form_type: legacy_return.form_identity.clone(),
    }
}

fn field_suffix(fields: &BTreeMap<String, String>, suffix: &str) -> Option<String> {
    fields
        .iter()
        .find(|(key, _)| {
            key.rsplit(':')
                .next()
                .is_some_and(|field| field.eq_ignore_ascii_case(suffix))
        })
        .map(|(_, value)| value.clone())
}

fn profile_key(tin: &str, branch: &str) -> Result<ProfileKey, LegacyMigrationError> {
    if tin.len() != 9 || !tin.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(LegacyMigrationError::InvalidTin(
            "must contain nine digits".to_string(),
        ));
    }
    if !(3..=5).contains(&branch.len()) || !branch.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(LegacyMigrationError::Database(DbError::Other(
            "legacy branch code must contain three to five digits".to_string(),
        )));
    }
    let branch = branch.parse::<u32>().map_err(|_| {
        LegacyMigrationError::Database(DbError::Other(
            "legacy branch code is not numeric".to_string(),
        ))
    })?;
    Ok(ProfileKey {
        tin: tin.to_string(),
        branch,
    })
}

fn profile_key_from_profile(profile: &TaxpayerProfile) -> Option<ProfileKey> {
    let tin = profile.tin.full();
    profile_key(tin.get(..9)?, &profile.tin.branch).ok()
}

fn new_profile(
    tin: &str,
    branch: &str,
    values: ProfileValues,
) -> Result<TaxpayerProfile, LegacyMigrationError> {
    let tin = Tin {
        segment1: tin[..3].to_string(),
        segment2: tin[3..6].to_string(),
        segment3: tin[6..9].to_string(),
        branch: format!("{:05}", branch.parse::<u32>().unwrap_or_default()),
    };
    let mut profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
        "id": null,
        "full_name": values.full_name,
        "tin": tin,
        "rdo_code": values.rdo_code,
        "line_of_business": values.line_of_business,
        "registered_address": values.registered_address,
        "zip_code": values.zip_code,
        "phone": values.phone,
        "email": values.email,
        "default_form_type": values.default_form_type
    }))?;
    profile.ensure_profile_version_ledger();
    Ok(profile)
}

fn fill_empty_fields(profile: &mut TaxpayerProfile, values: &ProfileValues) -> usize {
    let mut filled = 0;
    macro_rules! fill {
        ($field:ident) => {
            if profile.$field.trim().is_empty() && !values.$field.trim().is_empty() {
                profile.$field.clone_from(&values.$field);
                filled += 1;
            }
        };
    }
    fill!(full_name);
    fill!(rdo_code);
    fill!(line_of_business);
    fill!(registered_address);
    fill!(zip_code);
    fill!(phone);
    fill!(email);
    fill!(default_form_type);
    // Forms resolve identity through each version's COR facts, so an import
    // that only filled the flat fields would otherwise stay invisible there.
    for version in &mut profile.profile_versions {
        if matches!(
            version.status,
            TaxProfileVersionStatus::Confirmed | TaxProfileVersionStatus::Archived
        ) {
            continue;
        }
        let cor = &mut version.cor;
        if cor.registered_name.trim().is_empty() && !values.full_name.trim().is_empty() {
            cor.registered_name.clone_from(&values.full_name);
        }
        if cor.rdo_code.trim().is_empty() && !values.rdo_code.trim().is_empty() {
            cor.rdo_code.clone_from(&values.rdo_code);
        }
        if cor.line_of_business_description.trim().is_empty()
            && !values.line_of_business.trim().is_empty()
        {
            cor.line_of_business_description
                .clone_from(&values.line_of_business);
        }
        if cor.registered_address.trim().is_empty() && !values.registered_address.trim().is_empty()
        {
            cor.registered_address
                .clone_from(&values.registered_address);
        }
    }
    filled
}

/// Ordering key for "latest filing" when a return supplies the profile:
/// (year, position inside that year, amendment). A bare `YYYY` counts as
/// filed after December so a newer annual return wins over any earlier
/// month/quarter of an older year, and quarters map to their last month.
fn return_period_order(legacy_return: &LegacyReturn) -> (u32, u32, Option<u32>) {
    let period = &legacy_return.period;
    let (digits, quarter) = match period.find(['Q', 'q']) {
        Some(index) => {
            let quarter = match period[index + 1..].parse::<u32>() {
                Ok(quarter) if (1..=4).contains(&quarter) => quarter,
                _ => return (0, 0, legacy_return.amendment_version),
            };
            (period.get(..index).unwrap_or_default(), Some(quarter))
        }
        None => (period.as_str(), None),
    };
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return (0, 0, legacy_return.amendment_version);
    }
    let (year, sub_period) = match digits.len() {
        4 => (
            digits.parse::<u32>().unwrap_or_default(),
            quarter.map(|quarter| quarter * 3).unwrap_or(13),
        ),
        6 => match quarter {
            Some(quarter) => {
                let end_month = match digits[..2].parse::<u32>() {
                    Ok(month) if (1..=12).contains(&month) => month,
                    _ => return (0, 0, legacy_return.amendment_version),
                };
                let end_year = match digits[2..].parse::<u32>() {
                    Ok(year) if year > 0 => year,
                    _ => return (0, 0, legacy_return.amendment_version),
                };
                // MMYYYYQn counts backward from its fiscal year-end month.
                let Some(end_month_index) = end_year
                    .checked_mul(12)
                    .and_then(|year_month| year_month.checked_add(end_month - 1))
                else {
                    return (0, 0, legacy_return.amendment_version);
                };
                let months_before_year_end = (4 - quarter) * 3;
                let Some(quarter_end_index) = end_month_index.checked_sub(months_before_year_end)
                else {
                    return (0, 0, legacy_return.amendment_version);
                };
                (quarter_end_index / 12, quarter_end_index % 12 + 1)
            }
            None => (
                digits[2..].parse::<u32>().unwrap_or_default(),
                digits[..2].parse::<u32>().unwrap_or_default().clamp(1, 13),
            ),
        },
        8 => (
            digits[4..].parse::<u32>().unwrap_or_default(),
            digits[..2].parse::<u32>().unwrap_or_default().clamp(1, 13),
        ),
        _ => return (0, 0, legacy_return.amendment_version),
    };
    (year, sub_period, legacy_return.amendment_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("legacy-ebirforms")
    }

    fn test_profile(branch: &str) -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "User-entered name",
            "tin": {
                "segment1": "123",
                "segment2": "456",
                "segment3": "788",
                "branch": branch
            },
            "rdo_code": "",
            "line_of_business": "",
            "registered_address": "",
            "zip_code": "",
            "phone": "User-entered phone",
            "email": "",
            "default_form_type": ""
        }))
        .unwrap()
    }

    fn copy_fixture_returns(destination: &Path) {
        let source = fixture_root().join("IAF_RDO_Copy");
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
    }

    fn tree_hashes(root: &Path) -> Vec<(PathBuf, String)> {
        fn visit(root: &Path, directory: &Path, hashes: &mut Vec<(PathBuf, String)>) {
            for entry in fs::read_dir(directory).unwrap().flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    visit(root, &path, hashes);
                } else if entry.file_type().unwrap().is_file() {
                    let contents = fs::read(&path).unwrap();
                    hashes.push((
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        hex::encode(Sha256::digest(contents)),
                    ));
                }
            }
        }
        let mut hashes = Vec::new();
        visit(root, root, &mut hashes);
        hashes.sort();
        hashes
    }

    fn synthetic_return(period: &str, name: &str, sha256: &str) -> LegacyReturn {
        let fields = BTreeMap::from([
            ("frm1601c:txtTaxpayerName".to_string(), name.to_string()),
            ("frm1601c:txtRDOCode".to_string(), "039".to_string()),
            (
                "frm1601c:txtLineBus".to_string(),
                "Test Consulting".to_string(),
            ),
            (
                "frm1601c:txtAddress".to_string(),
                "123 Test Street".to_string(),
            ),
            ("frm1601c:txtZipCode".to_string(), "1100".to_string()),
            ("frm1601c:txtTelNum".to_string(), "09170000000".to_string()),
            (
                "frm1601c:txtEmail".to_string(),
                "legacy@example.test".to_string(),
            ),
        ]);
        LegacyReturn {
            source_path: PathBuf::from(format!("123456788000-1601Cv2018-{period}.xml")),
            raw_file: sha256.as_bytes().to_vec(),
            sha256: sha256.to_string(),
            tin: "123456788".to_string(),
            branch: "000".to_string(),
            form_identity: "1601Cv2018".to_string(),
            form_id: Some("1601C".to_string()),
            period: period.to_string(),
            amendment_version: None,
            email_copy: None,
            encrypted: true,
            fields,
        }
    }

    #[test]
    fn scan_parses_profiles_returns_issues_and_savefile_count() {
        let root = fixture_root();
        let before = tree_hashes(&root);
        let scan = scan_legacy_folder(&root).unwrap();
        let after = tree_hashes(&root);

        assert_eq!(before, after, "scanning must not modify source files");
        assert_eq!(scan.profiles.len(), 1);
        let profile = &scan.profiles[0];
        assert_eq!(profile.tin, "123456788");
        assert_eq!(profile.branch, "000");
        assert_eq!(profile.full_name, "Juan Dela Cruz Test");
        assert_eq!(profile.rdo_code, "039");
        assert_eq!(profile.line_of_business, "Test Consulting Services");
        assert_eq!(profile.registered_address, "123 Test Street \u{00D1}");
        assert_eq!(profile.zip_code, "1100");
        assert_eq!(profile.phone, "09170000000");
        assert_eq!(profile.email, "legacy@example.test");
        assert_eq!(profile.default_form_type, "1601Cv2018");

        assert_eq!(scan.returns.len(), 3, "scan issues: {:?}", scan.issues);
        let original = scan
            .returns
            .iter()
            .find(|legacy_return| {
                legacy_return.amendment_version.is_none() && legacy_return.email_copy.is_none()
            })
            .unwrap();
        assert_eq!(original.tin, "123456788");
        assert_eq!(original.branch, "000");
        assert_eq!(original.form_identity, "1601Cv2018");
        assert_eq!(original.form_id.as_deref(), Some("1601Cv2018"));
        assert_eq!(original.period, "062025");
        assert!(!original.encrypted);
        assert_eq!(
            original.fields.get("frm1601c:txtTaxpayerName").unwrap(),
            "Juan Dela Cruz Test"
        );
        assert_eq!(
            original.fields.get("frm1601c:txtAddress").unwrap(),
            "123 Test Street, Quezon City"
        );
        assert!(
            original
                .raw_file
                .windows(b"</div>".len())
                .any(|window| window == b"</div>")
        );

        let amended = scan
            .returns
            .iter()
            .find(|legacy_return| legacy_return.amendment_version.is_some())
            .unwrap();
        assert_eq!(amended.period, "062025");
        assert_eq!(amended.amendment_version, Some(1));
        let email_copy = scan
            .returns
            .iter()
            .find(|legacy_return| legacy_return.email_copy.is_some())
            .unwrap();
        assert_eq!(email_copy.email_copy.as_deref(), Some("copy@example.test"));
        assert_eq!(
            email_copy.fields.get("txtEmail").unwrap(),
            "copy@example.test"
        );
        assert_eq!(scan.issues.len(), 1);
        assert!(scan.issues[0].reason.contains("invalid return filename"));
        assert_eq!(scan.savefile_count, 1);
    }

    #[test]
    fn scan_accepts_case_insensitive_return_folder_and_plain_final_copies() {
        let temp = TempDir::new().unwrap();
        let lower = temp.path().join("iaf_rdo_copy");
        copy_fixture_returns(&lower);
        let scan = scan_legacy_folder(&lower).unwrap();
        assert_eq!(scan.returns.len(), 3, "scan issues: {:?}", scan.issues);

        let plain_root = TempDir::new().unwrap();
        let plain_folder = plain_root.path().join("IAF_RDO_Copy");
        fs::create_dir_all(&plain_folder).unwrap();
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("sample-payloads")
                .join("1601C-062025.official.xml"),
            plain_folder.join("123456788000-1601Cv2018-062025.xml"),
        )
        .unwrap();
        let plain_scan = scan_legacy_folder(&plain_folder).unwrap();
        assert_eq!(
            plain_scan.returns.len(),
            1,
            "plain scan issues: {:?}",
            plain_scan.issues
        );
        assert!(!plain_scan.returns[0].encrypted);
    }

    #[test]
    fn profile_reads_undefined_fields_as_empty_and_rejects_zero_tin() {
        let path = Path::new("profile/12345678800000.xml");
        // Real official profile: XML declaration and every div on one line.
        let contents = "<?xml version='1.0'?>        <div>fn=DELA%20CRUZ%20JUAN%20TAMADfn=</div>        <div>tin1=000tin1=</div>        <div>tin2=000tin2=</div>        <div>tin3=000tin3=</div>        <div>tin4=00000tin4=</div>        <div>rdo=018rdo=</div>        <div>lob=undefinedlob=</div>        <div>regAddr=OLONGAPOregAddr=</div>        <div>zip=2200zip=</div>        <div>telNo=09123456789telNo=</div>        <div>email=a@b.testemail=</div>        <div>confirmtin1=000confirmtin1=</div>        <div>formType=2200Tv2020formType=</div>                All Rights Reserved BIR 2014.";
        let profile =
            parse_profile_contents(contents, path, "123456788".to_string(), "00000".to_string())
                .unwrap();
        assert_eq!(profile.full_name, "DELA CRUZ JUAN TAMAD");
        assert_eq!(profile.line_of_business, "");
        assert_eq!(profile.registered_address, "OLONGAPO");

        let zero = parse_profile_contents(
            contents,
            Path::new("profile/00000000000000.xml"),
            "000000000".to_string(),
            "00000".to_string(),
        );
        assert!(zero.is_err_and(|reason| reason.contains("all zeros")));
    }

    #[test]
    fn scan_reports_bad_files_without_aborting() {
        let temp = TempDir::new().unwrap();
        let folder = temp.path().join("IAF_RDO_Copy");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("123456788000-1601Cv2018-062025.xml"),
            b"not a valid encrypted return",
        )
        .unwrap();
        fs::write(folder.join("readme.txt"), b"not xml").unwrap();

        let scan = scan_legacy_folder(&folder).unwrap();
        assert!(scan.returns.is_empty());
        assert_eq!(scan.issues.len(), 2);
        assert!(
            scan.issues
                .iter()
                .any(|issue| issue.reason.contains("encrypted"))
        );
        assert!(
            scan.issues
                .iter()
                .any(|issue| issue.reason.contains("not an XML"))
        );
        assert!(matches!(
            scan_legacy_folder(&temp.path().join("unrelated")),
            Err(LegacyMigrationError::NotALegacyFolder(_))
        ));
    }

    #[test]
    fn profile_key_reports_invalid_tin() {
        assert!(matches!(
            profile_key("12345678a", "000"),
            Err(LegacyMigrationError::InvalidTin(_))
        ));
    }

    #[test]
    fn apply_imports_raw_returns_idempotently_without_creating_drafts_or_submissions() {
        let scan = scan_legacy_folder(&fixture_root()).unwrap();
        let db = Database::open_ephemeral().unwrap();
        let report = apply_legacy_migration(&db, &scan).unwrap();
        assert_eq!(report.profiles_created, 1);
        assert_eq!(report.profiles_updated, 0);
        assert_eq!(report.returns_imported, 3);
        assert_eq!(report.issues.len(), 1);

        let profile = db.list_profiles().unwrap().remove(0);
        assert_eq!(profile.full_name, "Juan Dela Cruz Test");
        assert_eq!(profile.tin.branch, "00000");
        assert_eq!(profile.registered_address, "123 Test Street \u{00D1}");
        assert_eq!(profile.default_form_type, "1601Cv2018");
        assert!(profile.id.is_some(), "the migrated profile should be saved");
        let version = profile
            .profile_versions
            .first()
            .expect("the migrated profile should have a version");
        assert_eq!(version.effective_from, None);
        assert_eq!(
            version.status,
            crate::profile::TaxProfileVersionStatus::NeedsReview
        );

        // The user confirms the effective date in Profile Manager; Forms Sets follow from that.
        let current_year = chrono::Local::now()
            .format("%Y")
            .to_string()
            .parse::<i32>()
            .unwrap();
        let forms_set_rows: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM per_year_forms WHERE tin = ?1 AND taxable_year = ?2",
                rusqlite::params![profile.tin.full(), current_year],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            forms_set_rows == 0,
            "an undated profile must not have a current-year Forms Set yet"
        );

        let rows = db
            .list_legacy_filed_returns_for_tin("123456788000")
            .unwrap();
        assert_eq!(rows.len(), 3);
        for legacy_return in &scan.returns {
            let row = rows
                .iter()
                .find(|row| row.sha256 == legacy_return.sha256)
                .unwrap();
            assert_eq!(row.raw_file, legacy_return.raw_file);
            assert_eq!(row.tin, "12345678800000");
        }

        let repeated = apply_legacy_migration(&db, &scan).unwrap();
        assert_eq!(repeated.profiles_created, 0);
        assert_eq!(repeated.profiles_updated, 0);
        assert_eq!(repeated.profiles_unchanged, 1);
        assert_eq!(repeated.returns_imported, 0);
        assert_eq!(repeated.returns_already_present, 3);
        for table in ["form_drafts", "submissions", "job_queue"] {
            let count: i64 = db
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} must not receive migrated returns");
        }
    }

    #[test]
    fn apply_fills_only_empty_fields_and_matches_numeric_branches() {
        let scan = scan_legacy_folder(&fixture_root()).unwrap();
        let db = Database::open_ephemeral().unwrap();
        let profile = test_profile("00000");
        db.save_profile(profile).unwrap();

        let report = apply_legacy_migration(&db, &scan).unwrap();
        assert_eq!(report.profiles_created, 0);
        assert_eq!(report.profiles_updated, 1);
        assert_eq!(report.profile_fields_filled, 6);
        let saved = db.list_profiles().unwrap().remove(0);
        assert_eq!(saved.full_name, "User-entered name");
        assert_eq!(saved.phone, "User-entered phone");
        assert_eq!(saved.rdo_code, "039");
        assert_eq!(saved.registered_address, "123 Test Street \u{00D1}");
        assert_eq!(saved.email, "legacy@example.test");
    }

    #[test]
    fn apply_preserves_confirmed_cor_facts_and_imports_returns() {
        let scan = scan_legacy_folder(&fixture_root()).unwrap();
        let db = Database::open_ephemeral().unwrap();
        let mut profile = db.save_profile(test_profile("00000")).unwrap();
        profile.profile_versions[0].status = TaxProfileVersionStatus::Confirmed;
        profile.profile_versions[0].effective_from = chrono::NaiveDate::from_ymd_opt(2025, 1, 1);
        profile.profile_versions[0].needs_effective_date_review = false;
        let versions = profile.profile_versions.clone();
        assert!(versions[0].cor.rdo_code.is_empty());
        // Seed an existing confirmed record without going through confirmation UI.
        db.conn
            .execute(
                "UPDATE profiles SET data_json = ?1 WHERE id = ?2",
                rusqlite::params![serde_json::to_string(&profile).unwrap(), profile.id],
            )
            .unwrap();

        let report = apply_legacy_migration(&db, &scan).unwrap();
        assert_eq!(report.returns_imported, 3);
        assert_eq!(report.profiles_updated, 1);
        let saved = db.list_profiles().unwrap().remove(0);
        assert_eq!(
            serde_json::to_value(&saved.profile_versions).unwrap(),
            serde_json::to_value(&versions).unwrap()
        );
        assert_eq!(saved.rdo_code, "039");
        assert_eq!(
            apply_legacy_migration(&db, &scan).unwrap().returns_imported,
            0
        );
    }

    #[test]
    fn quarterly_return_order_uses_quarter_end_month() {
        for quarter in 1..=4 {
            for period in [format!("2025Q{quarter}"), format!("122025Q{quarter}")] {
                let quarterly = synthetic_return(&period, "Quarterly", "quarterly");
                for month in 1..=12 {
                    let monthly =
                        synthetic_return(&format!("{month:02}2025"), "Monthly", "monthly");
                    assert_eq!(
                        return_period_order(&quarterly).cmp(&return_period_order(&monthly)),
                        (quarter * 3).cmp(&month),
                        "{period} versus month {month}"
                    );
                }
            }
        }
        let db = Database::open_ephemeral().unwrap();
        let scan = LegacyScan {
            returns: vec![
                synthetic_return("022025", "February", "february"),
                synthetic_return("2025Q1", "Quarter One", "quarter-one"),
            ],
            ..LegacyScan::default()
        };
        apply_legacy_migration(&db, &scan).unwrap();
        assert_eq!(db.list_profiles().unwrap()[0].full_name, "Quarter One");
    }

    #[test]
    fn quarterly_return_order_rejects_invalid_quarters() {
        for period in [
            "2025Q4294967295",
            "2025Q0",
            "2025Q5",
            "2025Qx",
            "002025Q1",
            "132025Q4",
            "010000Q1",
        ] {
            let legacy_return = synthetic_return(period, "Invalid quarter", "invalid-quarter");
            assert_eq!(
                return_period_order(&legacy_return),
                (0, 0, None),
                "{period}"
            );
        }
    }

    #[test]
    fn six_digit_quarter_order_counts_back_from_year_end_month() {
        for (period, expected) in [
            ("062025Q4", (2025, 6)),
            ("062025Q1", (2024, 9)),
            ("062025Q2", (2024, 12)),
            ("062025Q3", (2025, 3)),
        ] {
            let legacy_return = synthetic_return(period, "Fiscal quarter", period);
            assert_eq!(
                return_period_order(&legacy_return),
                (expected.0, expected.1, None),
                "{period}"
            );
        }
    }

    #[test]
    fn fiscal_quarter_order_derives_profile_from_latest_endpoint() {
        for (quarter_period, quarter_name, comparison_period, comparison_name, expected_name) in [
            ("062025Q4", "Fiscal Q4", "072025", "July", "July"),
            ("062025Q1", "Fiscal Q1", "122024", "December", "December"),
        ] {
            let db = Database::open_ephemeral().unwrap();
            let scan = LegacyScan {
                returns: vec![
                    synthetic_return(quarter_period, quarter_name, "fiscal-quarter"),
                    synthetic_return(comparison_period, comparison_name, "comparison-period"),
                ],
                ..LegacyScan::default()
            };
            apply_legacy_migration(&db, &scan).unwrap();
            assert_eq!(
                db.list_profiles().unwrap()[0].full_name,
                expected_name,
                "{quarter_period} versus {comparison_period}"
            );
        }
    }

    #[test]
    fn profile_tin_rename_and_delete_cascade_to_legacy_returns() {
        let scan = scan_legacy_folder(&fixture_root()).unwrap();
        let db = Database::open_ephemeral().unwrap();
        let profile = test_profile("000");
        db.save_profile(profile).unwrap();

        apply_legacy_migration(&db, &scan).unwrap();
        let mut profile = db.list_profiles().unwrap().remove(0);
        profile.tin.segment1 = "987".to_string();
        db.save_profile(profile).unwrap();
        assert_eq!(
            db.list_legacy_filed_returns_for_tin("987456788000")
                .unwrap()
                .len(),
            3
        );

        db.delete_profile("987456788000").unwrap();
        assert!(
            db.list_legacy_filed_returns_for_tin("987456788000")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn apply_derives_missing_profiles_from_the_latest_return() {
        let db = Database::open_ephemeral().unwrap();
        let scan = LegacyScan {
            returns: vec![
                synthetic_return("062024", "Older Test Name", "older"),
                synthetic_return("062025", "Latest Test Name", "latest"),
            ],
            ..LegacyScan::default()
        };
        let report = apply_legacy_migration(&db, &scan).unwrap();
        assert_eq!(report.profiles_created, 1);
        let profile = db.list_profiles().unwrap().remove(0);
        assert_eq!(profile.full_name, "Latest Test Name");
        assert_eq!(profile.tin.branch, "00000");
        assert_eq!(profile.rdo_code, "039");
        assert_eq!(profile.default_form_type, "1601Cv2018");
    }
}
