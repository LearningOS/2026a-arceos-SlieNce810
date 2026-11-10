#![no_std]

use core::ptr::NonNull;

use allocator::{BaseAllocator, ByteAllocator, PageAllocator};

/// Early memory allocator
/// Use it before formal bytes-allocator and pages-allocator can work!
/// This is a double-end memory range:
/// - Alloc bytes forward
/// - Alloc pages backward
///
/// [ bytes-used | avail-area | pages-used ]
/// |            | -->    <-- |            |
/// start       b_pos        p_pos       end
///
/// For bytes area, 'count' records number of allocations.
/// When it goes down to ZERO, free bytes-used area.
/// For pages area, it will never be freed!
///
pub struct EarlyAllocator<const SIZE: usize> {
    start: usize,
    b_pos: usize,
    p_pos: usize,
    end: usize,
    count: usize,
}

impl<const SIZE: usize> EarlyAllocator<SIZE> {
    pub const fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            b_pos: 0,
            p_pos: 0,
            count: 0,
        }
    }
}

impl<const SIZE: usize> BaseAllocator for EarlyAllocator<SIZE> {
    fn init(&mut self, start: usize, size: usize) {
        self.start = start;
        self.end = start + size;
        self.b_pos = start;
        self.p_pos = self.end;
        self.count = 0;
    }

    fn add_memory(&mut self, start: usize, size: usize) -> allocator::AllocResult {
        // 1. 检查参数及地址溢出
        if size == 0 {
            return Err(allocator::AllocError::InvalidParam);
        }
        let new_end = start
            .checked_add(size)
            .ok_or(allocator::AllocError::InvalidParam)?;
        // 2. 检查新区域是否与原区域重叠
        if start < self.end && new_end > self.start {
            return Err(allocator::AllocError::MemoryOverlap);
        }
        // 3. 必须从原内存池的末尾开始拓展
        if start != self.end {
            return Err(allocator::AllocError::InvalidParam);
        }
        // 4. 如果已用页分配，不能直接移动 p_pos
        if self.p_pos != self.end {
            return Err(allocator::AllocError::InvalidParam);
        }
        // 5. 更新边界
        self.end = new_end;
        self.p_pos = new_end;
        Ok(())
    }
}

impl<const SIZE: usize> ByteAllocator for EarlyAllocator<SIZE> {
    fn alloc(
        &mut self,
        layout: core::alloc::Layout,
    ) -> allocator::AllocResult<core::ptr::NonNull<u8>> {
        // 1. 计算满足对齐要求的起始地址
        let align = layout.align();

        let alloc_start = self.b_pos
            .checked_add(align - 1)
            .map(|addr| addr & !(align - 1)) 
            .ok_or(allocator::AllocError::NoMemory)?;
        // 2. 计算结束地址
        let alloc_end = alloc_start
            .checked_add(layout.size())
            .ok_or(allocator::AllocError::NoMemory)?;
        // 3. 检查是否与页分配区域重叠
        if alloc_end > self.p_pos {
            return Err(allocator::AllocError::NoMemory);
        }
        // 4. 将地址转换为非空指针
        let ptr = NonNull::new(alloc_start as *mut u8)
            .ok_or(allocator::AllocError::NoMemory)?;
        // 5. 更新字节分配器状态
        self.b_pos = alloc_end;
        self.count += 1;
        
        Ok(ptr)
    }

    fn dealloc(&mut self, pos: core::ptr::NonNull<u8>, layout: core::alloc::Layout) {
        if self.count > 0 {
            self.count -= 1;
        }
        if self.count == 0 {
            self.b_pos = self.start;
        }
    }

    fn total_bytes(&self) -> usize {
        self.end - self.start
    }

    fn used_bytes(&self) -> usize {
        self.b_pos - self.start
    }

    fn available_bytes(&self) -> usize {
        self.p_pos - self.b_pos
    }
}

impl<const SIZE: usize> PageAllocator for EarlyAllocator<SIZE> {
    const PAGE_SIZE: usize = SIZE;

    fn alloc_pages(
        &mut self,
        num_pages: usize,
        align_pow2: usize,
    ) -> allocator::AllocResult<usize> {
        // 1. 参数检查 
        if SIZE == 0 
            || num_pages == 0
            || !align_pow2.is_power_of_two() {
            return Err(allocator::AllocError::InvalidParam);
        }
        // 2. 计算申请的总字节数
        let size = num_pages
            .checked_mul(SIZE)
            .ok_or(allocator::AllocError::InvalidParam)?;
        // 3. 从高地址向低地址计算起点
        let raw_start = self.p_pos
            .checked_sub(size)
            .ok_or(allocator::AllocError::NoMemory)?; 
        // 4. 向下对齐
        let alloc_start = raw_start & !(align_pow2 - 1);
        // 5. 不允许侵入字节分配区域
        if alloc_start < self.b_pos {
            return Err(allocator::AllocError::NoMemory);
        }
        // 6. 更新页面分配游标
        self.p_pos = alloc_start;
        // 7. 返回起始地址
        Ok(alloc_start)
    }

    fn dealloc_pages(&mut self, pos: usize, num_pages: usize) {
       
    }

    fn total_pages(&self) -> usize {
        (self.end - self.start) / SIZE
    }

    fn used_pages(&self) -> usize {
        (self.end - self.p_pos) / SIZE
    }

    fn available_pages(&self) -> usize {
        (self.p_pos - self.b_pos) / SIZE
    }
}