use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
    pub endpoint: String,
}
impl Paths {
    pub fn new(root: Option<PathBuf>) -> Result<Self> {
        let root = root
            .or_else(|| std::env::var_os("OMNIDOWNLOADER_HOME").map(PathBuf::from))
            .or_else(|| dirs::data_local_dir().map(|p| p.join("OmniDownloader")))
            .context("Cannot find a data directory; supply --data-dir")?;
        std::fs::create_dir_all(&root)?;
        secure_dir(&root)?;
        let root = std::fs::canonicalize(root)?;
        let hash = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
        #[cfg(windows)]
        let endpoint = format!("omnidownloader-{}", &hash[..24]);
        #[cfg(not(windows))]
        let endpoint = {
            let directory = format!(
                "omnidownloader-{}-{}",
                unsafe { libc::geteuid() },
                &hash[..16]
            );
            let mut runtime = std::env::temp_dir().join(&directory);
            // macOS sockaddr_un is limited to 104 bytes including its terminator.
            if runtime
                .join("worker.sock")
                .as_os_str()
                .as_encoded_bytes()
                .len()
                >= 104
            {
                runtime = PathBuf::from("/tmp").join(directory);
            }
            std::fs::create_dir_all(&runtime)?;
            secure_dir(&runtime)?;
            runtime.join("worker.sock").to_string_lossy().into_owned()
        };
        for name in ["accounts", "plugins"] {
            let p = root.join(name);
            std::fs::create_dir_all(&p)?;
            secure_dir(&p)?;
        }
        Ok(Self { root, endpoint })
    }
    pub fn token(&self) -> Result<String> {
        Ok(std::fs::read_to_string(self.root.join("worker.token"))?
            .trim()
            .to_string())
    }
    pub fn account_file(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.root.join("accounts").join(format!("{name}.txt")))
    }
}
pub fn validate_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "Names must contain 1–64 letters, digits, hyphens, or underscores"
    );
    Ok(())
}
pub fn secure_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        anyhow::ensure!(
            path.symlink_metadata()?.file_type().is_dir(),
            "Data directory must be a real directory, not a symbolic link"
        );
        anyhow::ensure!(
            path.symlink_metadata()?.uid() == unsafe { libc::geteuid() },
            "Data directory must belong to this user"
        );
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
                SE_FILE_OBJECT,
            },
            Security::{
                GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
                PROTECTED_DACL_SECURITY_INFORMATION,
            },
        };
        let descriptor: Vec<u16> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)\0"
            .encode_utf16()
            .collect();
        let mut sd = std::ptr::null_mut();
        unsafe {
            anyhow::ensure!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    descriptor.as_ptr(),
                    1,
                    &mut sd,
                    std::ptr::null_mut()
                ) != 0,
                "Cannot create data-directory permissions"
            );
            let mut acl = std::ptr::null_mut();
            let mut present = 0;
            let mut defaulted = 0;
            let valid = GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted);
            let mut name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let code = if valid != 0 {
                SetNamedSecurityInfoW(
                    name.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    acl,
                    std::ptr::null_mut(),
                )
            } else {
                1
            };
            LocalFree(sd);
            anyhow::ensure!(
                code == 0,
                "Cannot protect data directory (Windows error {code})"
            );
        }
    }
    Ok(())
}
