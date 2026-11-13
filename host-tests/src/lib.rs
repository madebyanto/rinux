extern crate alloc;

#[path = "../../src/mm/frame.rs"]
pub mod frame;
#[path = "../../src/mm/heap.rs"]
pub mod heap;
#[path = "../../src/drivers/ps2.rs"]
pub mod ps2;
#[path = "../../src/drivers/ata.rs"]
pub mod ata;
#[path = "../../src/proc/pid.rs"]
pub mod pid;
#[path = "../../src/proc/queue.rs"]
pub mod queue;
#[path = "../../src/proc/reap.rs"]
pub mod reap;
#[path = "../../src/user/range.rs"]
pub mod user_range;
#[path = "../../src/syscall/abi.rs"]
pub mod syscall_abi;
#[path = "../../src/arch/x86_64/descriptor.rs"]
pub mod descriptor;

pub mod fs;
#[path = "../../src/proc/wait.rs"]
pub mod wait;
#[path = "../../src/proc/wait_queue.rs"]
pub mod wait_queue;
