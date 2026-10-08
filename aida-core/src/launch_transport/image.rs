//! Independently copied, sealed executable bytes; paths are selection inputs only.
// Already Linux-only via mod.rs; explicit for the Windows-reachability ratchet.
#![cfg(target_os = "linux")]
// trace:TASK-1612 | ai:codex
// trace:BUG-1808 | ai:codex
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

pub(super) const SEALS: i32 =
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL | 0x0020; // F_SEAL_EXEC
const MAX_IMAGE: u64 = 512 * 1024 * 1024;
const MFD_EXEC: u32 = 0x0010;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageIdentity {
    pub digest: [u8; 32],
    pub size: u64,
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub owner: (u32, u32),
    pub source_mode: u32,
    pub source_owner: (u32, u32),
    pub machine: u16,
    pub interpreter: Option<String>,
    pub interpreter_identity: Option<(u64, u64)>,
    pub source_identity: (u64, u64),
}

/// Not Clone and no executable descriptor escape. The source's privileges and
/// mount access are checked on the opened object, before copying strips metadata.
#[derive(Debug)]
pub struct SealedExecImage {
    pub(super) file: File,
    identity: ImageIdentity,
}
impl SealedExecImage {
    /// `selected_digest` is the caller's prepared selection, not a path hash
    /// recomputed after a swap. A mismatch never silently chooses new bytes.
    pub fn prepare(path: &Path, selected_digest: [u8; 32]) -> Result<Self> {
        super::require_supported_build()?;
        let source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)
            .with_context(|| format!("open selected executable {}", path.display()))?;
        source_policy(&source)
            .with_context(|| format!("unsupported executable {}", path.display()))?;
        let before = source.metadata()?;
        ensure!(before.len() <= MAX_IMAGE, "executable exceeds image bound");
        let mut bytes = Vec::new();
        (&source).take(MAX_IMAGE + 1).read_to_end(&mut bytes)?;
        // Recheck opened-object privileges after the copy too: metadata may
        // change between the first policy check and the initial stat sample.
        source_policy(&source)?;
        let after = source.metadata()?;
        ensure!(
            before.len() == after.len()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec()
                && bytes.len() as u64 == before.len(),
            "selected executable changed during snapshot"
        );
        let file = sealed_bytes(&bytes, true)?;
        let sealed = read_bounded(&file, MAX_IMAGE)?;
        let digest: [u8; 32] = Sha256::digest(&sealed).into();
        ensure!(
            digest == selected_digest,
            "selected executable digest changed"
        );
        let (machine, interpreter) = elf_policy(&sealed)?;
        let interpreter_identity = if let Some(ref path) = interpreter {
            let loader = File::open(path).context("open trusted system loader")?;
            source_policy(&loader)?;
            let m = loader.metadata()?;
            ensure!(
                m.uid() == 0 && m.mode() & 0o022 == 0,
                "untrusted system loader permissions"
            );
            Some((m.dev(), m.ino()))
        } else {
            None
        };
        let meta = file.metadata()?;
        let identity = ImageIdentity {
            digest,
            size: meta.len(),
            device: meta.dev(),
            inode: meta.ino(),
            mode: meta.mode(),
            owner: (meta.uid(), meta.gid()),
            source_mode: before.mode(),
            source_owner: (before.uid(), before.gid()),
            machine,
            interpreter,
            interpreter_identity,
            source_identity: (before.dev(), before.ino()),
        };
        Ok(Self { file, identity })
    }
    pub fn identity(&self) -> &ImageIdentity {
        &self.identity
    }
    pub(super) fn verify(&self) -> Result<()> {
        verify_image(&self.file, &self.identity)
    }
}

pub(super) fn source_policy(source: &File) -> Result<()> {
    let m = source.metadata()?;
    ensure!(m.is_file(), "native executable must be a regular file");
    ensure!(m.mode() & 0o6000 == 0, "set-id executable is unsupported");
    let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { libc::fstatvfs(source.as_raw_fd(), &mut fs) } == 0,
        "cannot inspect executable mount"
    );
    ensure!(
        fs.f_flag & libc::ST_NOEXEC == 0,
        "executable is on a noexec mount"
    );
    // faccessat2 + AT_EMPTY_PATH checks this descriptor, including effective
    // credentials and ACLs. Old kernels are unsupported, never path-fallback.
    ensure!(unsafe {
        libc::syscall(libc::SYS_faccessat2, source.as_raw_fd(), c"".as_ptr(), libc::X_OK,
            libc::AT_EMPTY_PATH | libc::AT_EACCESS)
    } == 0, "opened executable is not executable with effective credentials (or kernel lacks faccessat2)");
    let n = unsafe {
        libc::fgetxattr(
            source.as_raw_fd(),
            c"security.capability".as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    if n >= 0 {
        bail!("file-capability executable is unsupported");
    }
    let err = std::io::Error::last_os_error().raw_os_error();
    ensure!(
        matches!(err, Some(libc::ENODATA) | Some(libc::ENOTSUP)),
        "cannot inspect executable capabilities"
    );
    Ok(())
}

pub(super) fn sealed_bytes(bytes: &[u8], executable: bool) -> Result<File> {
    // Require explicit executable-memfd kernel support (Linux 6.3+), no legacy
    // retry that might defeat vm.memfd_noexec policy.
    let flags = libc::MFD_CLOEXEC
        | libc::MFD_ALLOW_SEALING
        | if executable {
            MFD_EXEC
        } else {
            0x0008 /* MFD_NOEXEC_SEAL */
        };
    let fd = unsafe { libc::memfd_create(c"aida-private-image".as_ptr(), flags) };
    ensure!(
        fd >= 0,
        "executable sealed memfd profile unavailable: {}",
        std::io::Error::last_os_error()
    );
    let mut writer = unsafe { File::from_raw_fd(fd) };
    writer.write_all(bytes)?;
    ensure!(
        unsafe { libc::fchmod(fd, if executable { 0o700 } else { 0o600 }) } == 0,
        "cannot restrict sealed image mode"
    );
    ensure!(
        unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, SEALS) } == 0,
        "cannot seal image"
    );
    // Reopening this *owned descriptor* read-only is not reopening the mutable
    // selection path. Close the only writer before any fork/exec.
    let readonly = File::open(format!("/proc/self/fd/{fd}"))?;
    drop(writer);
    verify_sealed(&readonly)?;
    Ok(readonly)
}

pub(super) fn verify_sealed(file: &File) -> Result<()> {
    let fd = file.as_raw_fd();
    let seals = unsafe { libc::fcntl(fd, libc::F_GET_SEALS) };
    ensure!(
        seals >= 0 && seals & SEALS == SEALS,
        "missing immutable image seals"
    );
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    ensure!(
        flags >= 0 && flags & libc::O_ACCMODE == libc::O_RDONLY,
        "image descriptor is not read-only"
    );
    Ok(())
}
pub(super) fn read_bounded(file: &File, bound: u64) -> Result<Vec<u8>> {
    let size = file.metadata()?.len();
    ensure!(size <= bound, "sealed payload exceeds bound");
    let mut bytes = vec![0; size as usize];
    file.read_exact_at(&mut bytes, 0)?;
    Ok(bytes)
}
pub(super) fn verify_image(file: &File, id: &ImageIdentity) -> Result<()> {
    verify_sealed(file)?;
    let m = file.metadata()?;
    ensure!(
        m.mode() == id.mode && (m.uid(), m.gid()) == id.owner,
        "sealed executable credential metadata changed"
    );
    source_policy(file)?;
    ensure!(
        (m.dev(), m.ino(), m.len()) == (id.device, id.inode, id.size),
        "image descriptor identity mismatch"
    );
    let bytes = read_bounded(file, MAX_IMAGE)?;
    ensure!(
        <[u8; 32]>::from(Sha256::digest(&bytes)) == id.digest,
        "sealed image digest mismatch"
    );
    let (machine, interpreter) = elf_policy(&bytes)?;
    ensure!(
        machine == id.machine && interpreter == id.interpreter,
        "sealed ELF profile changed"
    );
    if let Some(path) = &id.interpreter {
        let m = std::fs::metadata(path)?;
        ensure!(
            Some((m.dev(), m.ino())) == id.interpreter_identity,
            "trusted loader identity changed"
        );
    }
    Ok(())
}

fn u16_at(b: &[u8], i: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(i..i + 2).context("truncated ELF")?.try_into()?,
    ))
}
fn u32_at(b: &[u8], i: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(i..i + 4).context("truncated ELF")?.try_into()?,
    ))
}
fn u64_at(b: &[u8], i: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        b.get(i..i + 8).context("truncated ELF")?.try_into()?,
    ))
}
fn section(b: &[u8], start: u64, len: u64) -> Result<&[u8]> {
    let end = start.checked_add(len).context("ELF offset overflow")?;
    b.get(usize::try_from(start)?..usize::try_from(end)?)
        .context("ELF range outside image")
}

/// Narrow 64-bit little-endian native ELF profile. Reject all RPATH/RUNPATH,
/// dynamic filters/audit and pathname DT_NEEDED. System loader/libs are trusted
/// platform components; this does not purport to seal a Git callback runtime.
fn elf_policy(b: &[u8]) -> Result<(u16, Option<String>)> {
    ensure!(
        b.get(..7) == Some(b"\x7fELF\x02\x01\x01"),
        "unsupported image: native 64-bit ELF required; scripts/wrappers need an adapter"
    );
    ensure!(matches!(u16_at(b, 16)?, 2 | 3), "unsupported ELF type");
    let machine = u16_at(b, 18)?;
    ensure!(
        (cfg!(target_arch = "x86_64") && machine == 62)
            || (cfg!(target_arch = "aarch64") && machine == 183),
        "unsupported ELF architecture"
    );
    ensure!(
        u32_at(b, 20)? == 1 && u16_at(b, 52)? == 64 && u16_at(b, 54)? == 56,
        "unsupported ELF headers"
    );
    let offset = u64_at(b, 32)?;
    let count = u16_at(b, 56)?;
    ensure!(count > 0 && count < 1024, "unsupported ELF program table");
    let mut interp = None;
    let mut dynamic = None;
    let mut loads = Vec::new();
    for i in 0..u64::from(count) {
        let p = section(
            b,
            offset.checked_add(i * 56).context("ELF table overflow")?,
            56,
        )?;
        let start = u64_at(p, 8)?;
        let size = u64_at(p, 32)?;
        let segment = section(b, start, size)?;
        match u32_at(p, 0)? {
            1 => loads.push((u64_at(p, 16)?, start, size)),
            2 => {
                ensure!(dynamic.is_none(), "multiple dynamic segments");
                dynamic = Some(segment);
            }
            3 => {
                ensure!(interp.is_none(), "multiple ELF interpreters");
                let path = std::ffi::CStr::from_bytes_with_nul(segment)?.to_str()?;
                let expected = if machine == 62 {
                    "/lib64/ld-linux-x86-64.so.2"
                } else {
                    "/lib/ld-linux-aarch64.so.1"
                };
                ensure!(path == expected, "unknown or vendor-relative ELF loader");
                interp = Some(path.to_owned());
            }
            _ => {}
        }
    }
    if let Some(entries) = dynamic {
        ensure!(entries.len() % 16 == 0, "malformed ELF dynamic table");
        let mut strtab = None;
        let mut strsz = None;
        let mut needed = Vec::new();
        let mut terminated = false;
        for d in entries.chunks_exact(16) {
            let tag = u64_at(d, 0)?;
            let value = u64_at(d, 8)?;
            match tag {
                0 => {
                    terminated = true;
                    break;
                }
                1 => needed.push(value),
                5 => strtab = Some(value),
                10 => strsz = Some(value),
                15 | 29 | 0x6ffffefa | 0x6ffffefb | 0x6ffffefc | 0x7fffffff | 0x7ffffffd => {
                    bail!("ELF runtime path/audit/filter requires an unsupported adapter")
                }
                _ => {}
            }
        }
        ensure!(terminated, "unterminated ELF dynamic table");
        if !needed.is_empty() {
            ensure!(
                interp.is_some(),
                "dynamic dependencies without system interpreter"
            );
            let address = strtab.context("missing ELF string table")?;
            let size = strsz.context("missing ELF string size")?;
            let (addr, start, _) = loads
                .iter()
                .find(|(addr, _, len)| {
                    address >= *addr
                        && address
                            .checked_add(size)
                            .is_some_and(|end| end <= addr.saturating_add(*len))
                })
                .context("unmapped ELF string table")?;
            let strings = section(b, start + (address - *addr), size)?;
            for off in needed {
                let tail = strings
                    .get(usize::try_from(off)?..)
                    .context("bad ELF dependency")?;
                let name = std::ffi::CStr::from_bytes_until_nul(tail)?.to_bytes();
                ensure!(
                    !name.is_empty() && !name.contains(&b'/'),
                    "non-system ELF dependency path"
                );
            }
        }
    }
    Ok((machine, interp))
}

pub(super) fn dup_high(fd: RawFd) -> Result<File> {
    let copied = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 16) };
    ensure!(
        copied >= 0,
        "duplicate owned descriptor: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { File::from_raw_fd(copied) })
}
