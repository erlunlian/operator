use std::time::Instant;

/// Snapshot of process-level performance metrics.
#[derive(Clone, Debug)]
pub struct ProcessMetrics {
    /// Resident (physical) memory in bytes.
    pub resident_bytes: u64,
    /// Virtual memory in bytes.
    pub virtual_bytes: u64,
    /// Number of active threads in this process.
    pub thread_count: u32,
    /// Number of terminal tabs across all workspaces.
    pub terminal_count: usize,
    /// Number of workspaces.
    pub workspace_count: usize,
    /// Time at which this snapshot was collected.
    pub sampled_at: Instant,
    /// Per-subsystem memory breakdown (estimated).
    pub subsystems: SubsystemMetrics,
}

/// Per-subsystem memory estimates collected from app state.
#[derive(Clone, Debug, Default)]
pub struct SubsystemMetrics {
    /// Estimated terminal grid memory (all terminals combined).
    pub terminal_grid_bytes: usize,
    /// Per-terminal breakdown: (total_lines, columns, estimated_bytes).
    pub terminal_details: Vec<(usize, usize, usize)>,
    /// Git diff panel estimated bytes (staged + unstaged DiffFiles + source_lines).
    pub git_diff_bytes: usize,
    /// Number of files in git diff panel.
    pub git_diff_files: usize,
    /// PR diff panel estimated bytes.
    pub pr_diff_bytes: usize,
    /// Number of files in PR diff panel.
    pub pr_diff_files: usize,
}

impl ProcessMetrics {
    /// Collect a new metrics snapshot from the OS.
    pub fn collect(terminal_count: usize, workspace_count: usize, subsystems: SubsystemMetrics) -> Self {
        let (resident_bytes, virtual_bytes, thread_count) = mach_task_info();
        Self {
            resident_bytes,
            virtual_bytes,
            thread_count,
            terminal_count,
            workspace_count,
            sampled_at: Instant::now(),
            subsystems,
        }
    }

    /// Format resident memory as a human-readable string.
    pub fn resident_display(&self) -> String {
        format_bytes(self.resident_bytes)
    }

    /// Format virtual memory as a human-readable string.
    pub fn virtual_display(&self) -> String {
        format_bytes(self.virtual_bytes)
    }

    /// Tracked subsystem total as estimated bytes.
    pub fn tracked_total(&self) -> usize {
        self.subsystems.terminal_grid_bytes
            + self.subsystems.git_diff_bytes
            + self.subsystems.pr_diff_bytes
    }

    /// Gap between RSS and tracked subsystems — the "unaccounted" memory.
    pub fn untracked_bytes(&self) -> u64 {
        (self.resident_bytes).saturating_sub(self.tracked_total() as u64)
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    }
}

/// Query macOS Mach APIs for (resident_size, virtual_size, thread_count).
#[cfg(target_os = "macos")]
fn mach_task_info() -> (u64, u64, u32) {
    use std::mem;

    // Mach types — from <mach/task_info.h>
    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct mach_task_basic_info {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [u32; 2],   // time_value_t (seconds, microseconds)
        system_time: [u32; 2], // time_value_t
        policy: i32,
        suspend_count: i32,
    }

    const MACH_TASK_BASIC_INFO: u32 = 20;
    const MACH_TASK_BASIC_INFO_COUNT: u32 =
        (mem::size_of::<mach_task_basic_info>() / mem::size_of::<u32>()) as u32;

    extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(
            target_task: u32,
            flavor: u32,
            task_info_out: *mut mach_task_basic_info,
            task_info_count: *mut u32,
        ) -> i32;
        fn task_threads(
            target_task: u32,
            act_list: *mut *mut u32,
            act_list_cnt: *mut u32,
        ) -> i32;
        fn vm_deallocate(target_task: u32, address: u64, size: u64) -> i32;
    }

    unsafe {
        let port = mach_task_self();

        // Get memory info
        let mut info: mach_task_basic_info = mem::zeroed();
        let mut count = MACH_TASK_BASIC_INFO_COUNT;
        let kr = task_info(port, MACH_TASK_BASIC_INFO, &mut info, &mut count);
        let (resident, virtual_sz) = if kr == 0 {
            (info.resident_size, info.virtual_size)
        } else {
            (0, 0)
        };

        // Get thread count
        let mut thread_list: *mut u32 = std::ptr::null_mut();
        let mut thread_count: u32 = 0;
        let kr = task_threads(port, &mut thread_list, &mut thread_count);
        if kr == 0 && !thread_list.is_null() {
            vm_deallocate(
                port,
                thread_list as u64,
                (thread_count as u64) * (mem::size_of::<u32>() as u64),
            );
        }

        (resident, virtual_sz, thread_count)
    }
}

#[cfg(not(target_os = "macos"))]
fn mach_task_info() -> (u64, u64, u32) {
    (0, 0, 0)
}
