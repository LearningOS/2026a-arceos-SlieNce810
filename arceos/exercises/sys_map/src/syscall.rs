#![allow(dead_code)]

use core::ffi::{c_void, c_char, c_int};
use axhal::arch::TrapFrame;
use axhal::trap::{register_trap_handler, SYSCALL};
use axerrno::LinuxError;
use axtask::current;
use axtask::TaskExtRef;
use axhal::paging::MappingFlags;
use arceos_posix_api as api;
use memory_addr::{PAGE_SIZE_4K, VirtAddrRange};

const SYS_IOCTL: usize = 29;
const SYS_OPENAT: usize = 56;
const SYS_CLOSE: usize = 57;
const SYS_READ: usize = 63;
const SYS_WRITE: usize = 64;
const SYS_WRITEV: usize = 66;
const SYS_EXIT: usize = 93;
const SYS_EXIT_GROUP: usize = 94;
const SYS_SET_TID_ADDRESS: usize = 96;
const SYS_MMAP: usize = 222;

const AT_FDCWD: i32 = -100;

/// Macro to generate syscall body
///
/// It will receive a function which return Result<_, LinuxError> and convert it to
/// the type which is specified by the caller.
#[macro_export]
macro_rules! syscall_body {
    ($fn: ident, $($stmt: tt)*) => {{
        #[allow(clippy::redundant_closure_call)]
        let res = (|| -> axerrno::LinuxResult<_> { $($stmt)* })();
        match res {
            Ok(_) | Err(axerrno::LinuxError::EAGAIN) => debug!(concat!(stringify!($fn), " => {:?}"),  res),
            Err(_) => info!(concat!(stringify!($fn), " => {:?}"), res),
        }
        match res {
            Ok(v) => v as _,
            Err(e) => {
                -e.code() as _
            }
        }
    }};
}

bitflags::bitflags! {
    #[derive(Debug)]
    /// permissions for sys_mmap
    ///
    /// See <https://github.com/bminor/glibc/blob/master/bits/mman.h>
    struct MmapProt: i32 {
        /// Page can be read.
        const PROT_READ = 1 << 0;
        /// Page can be written.
        const PROT_WRITE = 1 << 1;
        /// Page can be executed.
        const PROT_EXEC = 1 << 2;
    }
}

impl From<MmapProt> for MappingFlags {
    fn from(value: MmapProt) -> Self {
        let mut flags = MappingFlags::USER;
        if value.contains(MmapProt::PROT_READ) {
            flags |= MappingFlags::READ;
        }
        if value.contains(MmapProt::PROT_WRITE) {
            flags |= MappingFlags::WRITE;
        }
        if value.contains(MmapProt::PROT_EXEC) {
            flags |= MappingFlags::EXECUTE;
        }
        flags
    }
}

bitflags::bitflags! {
    #[derive(Debug)]
    /// flags for sys_mmap
    ///
    /// See <https://github.com/bminor/glibc/blob/master/bits/mman.h>
    struct MmapFlags: i32 {
        /// Share changes
        const MAP_SHARED = 1 << 0;
        /// Changes private; copy pages on write.
        const MAP_PRIVATE = 1 << 1;
        /// Map address must be exactly as requested, no matter whether it is available.
        const MAP_FIXED = 1 << 4;
        /// Don't use a file.
        const MAP_ANONYMOUS = 1 << 5;
        /// Don't check for reservations.
        const MAP_NORESERVE = 1 << 14;
        /// Allocation is for a stack.
        const MAP_STACK = 0x20000;
    }
}

#[register_trap_handler(SYSCALL)]
fn handle_syscall(tf: &TrapFrame, syscall_num: usize) -> isize {
    ax_println!("handle_syscall [{}] ...", syscall_num);
    let ret = match syscall_num {
        SYS_IOCTL => sys_ioctl(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _) as _,
        SYS_SET_TID_ADDRESS => sys_set_tid_address(tf.arg0() as _),
        SYS_OPENAT => sys_openat(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _, tf.arg3() as _),
        SYS_CLOSE => sys_close(tf.arg0() as _),
        SYS_READ => sys_read(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _),
        SYS_WRITE => sys_write(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _),
        SYS_WRITEV => sys_writev(tf.arg0() as _, tf.arg1() as _, tf.arg2() as _),
        SYS_EXIT_GROUP => {
            ax_println!("[SYS_EXIT_GROUP]: system is exiting ..");
            axtask::exit(tf.arg0() as _)
        },
        SYS_EXIT => {
            ax_println!("[SYS_EXIT]: system is exiting ..");
            axtask::exit(tf.arg0() as _)
        },
        SYS_MMAP => sys_mmap(
            tf.arg0() as _,
            tf.arg1() as _,
            tf.arg2() as _,
            tf.arg3() as _,
            tf.arg4() as _,
            tf.arg5() as _,
        ),
        _ => {
            ax_println!("Unimplemented syscall: {}", syscall_num);
            -LinuxError::ENOSYS.code() as _
        }
    };
    ret
}

#[allow(unused_variables)]
fn sys_mmap(
    addr: *mut usize,
    length: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    _offset: isize,
) -> isize {
    let result = (|| -> Result<usize, LinuxError> {
        // 1. 参数检查
        if length == 0 {
            return Err(LinuxError::EINVAL);
        }

        // 文件偏移必须非负且按页对齐
        if _offset < 0 || (_offset as usize) % PAGE_SIZE_4K != 0 {
            return Err(LinuxError::EINVAL);
        }

        // 2. 将映射长度向上对齐到 4 KiB
        let size = length
            .checked_add(PAGE_SIZE_4K - 1)
            .ok_or(LinuxError::ENOMEM)?
            & !(PAGE_SIZE_4K - 1);

        // 3. 将 prot 转换为 MappingFlags
        let map_flags: MappingFlags =
            MmapProt::from_bits_truncate(prot).into();

        // 4. 确定非零的 hint 地址
        let hint = if addr.is_null() {
            0x1000_0000usize
        } else {
            (addr as usize)
                .checked_add(PAGE_SIZE_4K - 1)
                .ok_or(LinuxError::ENOMEM)?
                & !(PAGE_SIZE_4K - 1)
        };

        // 5. 创建零初始化的文件缓冲区
        let mut buffer =alloc::vec![0u8; length];
        buffer
            .try_reserve_exact(length)
            .map_err(|_| LinuxError::ENOMEM)?;
        buffer.resize(length, 0);

        // 6. 从指定文件偏移读取内容
        let ret = api::sys_lseek(fd, _offset as _, 0); // whence = 0 即 SEEK_SET
        if ret < 0 {
            return Err(LinuxError::try_from((-ret) as i32).unwrap_or(LinuxError::EIO));
        }

        let mut total = 0usize;
        while total < length {
            let n = api::sys_read(
                fd,
                buffer[total..].as_mut_ptr() as *mut c_void,
                length - total,
            );
            if n < 0 {
                return Err(LinuxError::try_from((-n) as i32).unwrap_or(LinuxError::EIO));
            }
            if n == 0 {
                break; // EOF，buffer 剩余部分本来就是 0
            }
            total += n as usize;
        }

        //7. 获取当前任务的用户空间
        let curr = current();
        let mut aspace = curr.task_ext().aspace.lock();

        // 8. 构造地址搜索范围
        let limit = VirtAddrRange::from_start_size(
            aspace.base(),
            aspace.size(),
        );

        // 9. 查找可用虚拟地址
        let start = aspace
            .find_free_area(hint.into(), size, limit)
            .ok_or(LinuxError::ENOMEM)?;

        // 10. 分配物理页面并建立映射
        aspace
            .map_alloc(start, size, map_flags, true)
            .map_err(|_| LinuxError::ENOMEM)?;

        // 11. 将文件内容写入映射区域
        if let Err(err) = aspace.write(start, &buffer) {
            // 写入失败则回滚映射
            let _ = aspace.unmap(start, size);
            return Err(LinuxError::from(err));
        }

        // 12. 成功返回映射起始地址
        Ok(start.as_usize())
    })();

    // 13. 转换为 Linux syscall 返回格式
    match result {
        Ok(addr) => addr as isize,
        Err(err) => -err.code() as isize,
    }
}

fn sys_openat(dfd: c_int, fname: *const c_char, flags: c_int, mode: api::ctypes::mode_t) -> isize {
    assert_eq!(dfd, AT_FDCWD);
    api::sys_open(fname, flags, mode) as isize
}

fn sys_close(fd: i32) -> isize {
    api::sys_close(fd) as isize
}

fn sys_read(fd: i32, buf: *mut c_void, count: usize) -> isize {
    api::sys_read(fd, buf, count)
}

fn sys_write(fd: i32, buf: *const c_void, count: usize) -> isize {
    api::sys_write(fd, buf, count)
}

fn sys_writev(fd: i32, iov: *const api::ctypes::iovec, iocnt: i32) -> isize {
    unsafe { api::sys_writev(fd, iov, iocnt) }
}

fn sys_set_tid_address(tid_ptd: *const i32) -> isize {
    let curr = current();
    curr.task_ext().set_clear_child_tid(tid_ptd as _);
    curr.id().as_u64() as isize
}

fn sys_ioctl(_fd: i32, _op: usize, _argp: *mut c_void) -> i32 {
    ax_println!("Ignore SYS_IOCTL");
    0
}
