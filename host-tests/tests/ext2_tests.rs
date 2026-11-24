use hosttest::fs::ext2::bitmap::Bitmap;
use hosttest::fs::ext2::dir::{
    init_dot_dotdot_block, init_empty_dir_block, insert_dir_entry, remove_dir_entry,
};
use hosttest::fs::ext2::layout;
use hosttest::fs::ext2::structs::Superblock;
use hosttest::fs::ext2::{
    block_path, parse_dir_entry, BlockDevice, BlockPath, Ext2, Ext2Error,
};
use hosttest::fs::ramdisk::RamDisk;
use hosttest::fs::vfs::{
    DirCursor, FileType, Vfs, VfsError, O_CREAT, O_EXCL, O_RDWR, O_TRUNC,
};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static NEXT: AtomicU64 = AtomicU64::new(0);
static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();

fn image() -> Vec<u8> {
    IMAGE.get_or_init(build_image).clone()
}

fn build_image() -> Vec<u8> {
    let base = std::env::temp_dir().join(format!(
        "rinux-ext2-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("root");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(root.join("emptydir")).unwrap();
    fs::write(root.join("hello.txt"), b"hello from ext2\n").unwrap();
    fs::write(root.join("readme.txt"), b"rinux ext2 fixture\n").unwrap();
    fs::write(root.join("sub/nested.txt"), b"nested file\n").unwrap();
    let large: Vec<u8> = (0..300 * 1024).map(pattern).collect();
    fs::write(root.join("large.bin"), &large).unwrap();
    let image: PathBuf = base.join("image");
    let status = Command::new("mke2fs")
        .args(["-t", "ext2", "-b", "1024", "-I", "128", "-L", "rinux-test", "-F", "-d"])
        .arg(&root)
        .arg(&image)
        .arg("4096")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("mke2fs must be installed");
    assert!(status.success(), "mke2fs failed");
    let data = fs::read(&image).unwrap();
    fs::remove_dir_all(&base).ok();
    data
}

fn pattern(index: usize) -> u8 {
    index.wrapping_mul(31).wrapping_add(7) as u8
}

fn build_minimal_image(block_size: &str, inode_size: &str, blocks: &str) -> Vec<u8> {
    let base = std::env::temp_dir().join(format!(
        "rinux-ext2-min-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("hello.txt"), b"hello\n").unwrap();
    let image = base.join("image");
    let status = Command::new("mke2fs")
        .args([
            "-t",
            "ext2",
            "-b",
            block_size,
            "-I",
            inode_size,
            "-F",
            "-d",
        ])
        .arg(&root)
        .arg(&image)
        .arg(blocks)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("mke2fs must be installed");
    assert!(status.success(), "mke2fs failed");
    let data = fs::read(&image).unwrap();
    fs::remove_dir_all(&base).ok();
    data
}

struct MemoryDevice {
    data: Vec<u8>,
    reads: usize,
}

impl MemoryDevice {
    fn new(data: Vec<u8>) -> Self {
        Self { data, reads: 0 }
    }
}

impl BlockDevice for MemoryDevice {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Ext2Error> {
        let start = offset as usize;
        let end = start.checked_add(buffer.len()).ok_or(Ext2Error::Device)?;
        if end > self.data.len() {
            return Err(Ext2Error::Device);
        }
        buffer.copy_from_slice(&self.data[start..end]);
        self.reads += 1;
        Ok(())
    }
}

fn mount() -> Ext2<MemoryDevice> {
    Ext2::mount(MemoryDevice::new(image())).unwrap()
}

#[test]
fn mounts_and_parses_superblock() {
    let filesystem = mount();
    let superblock = filesystem.superblock();
    assert_eq!(superblock.volume_name(), "rinux-test");
    assert_eq!(superblock.block_size(), 1024);
    assert_eq!(superblock.inode_size(), 128);
    assert_eq!(superblock.blocks_count(), 4096);
    assert!(superblock.inodes_count() > 0);
}

#[test]
fn resolves_paths_including_dot_and_dotdot() {
    let mut filesystem = mount();
    assert_eq!(filesystem.lookup("/").unwrap(), 2);
    let hello = filesystem.lookup("/hello.txt").unwrap();
    let nested = filesystem.lookup("/sub/nested.txt").unwrap();
    let sub = filesystem.lookup("/sub").unwrap();
    assert_eq!(
        filesystem.stat_inode(hello).unwrap().kind,
        FileType::Regular
    );
    assert_eq!(
        filesystem.stat_inode(sub).unwrap().kind,
        FileType::Directory
    );
    assert_eq!(filesystem.stat_inode(nested).unwrap().size, 12);
    assert_eq!(filesystem.lookup("/sub/../hello.txt").unwrap(), hello);
    assert_eq!(filesystem.lookup("/./hello.txt").unwrap(), hello);
    assert_eq!(filesystem.lookup("/sub/./nested.txt").unwrap(), nested);
    assert_eq!(filesystem.lookup("/missing"), Err(Ext2Error::NotFound));
    assert_eq!(
        filesystem.lookup("/hello.txt/nope"),
        Err(Ext2Error::NotDirectory)
    );
}

#[test]
fn reads_files_across_direct_singly_and_doubly_indirect() {
    let mut filesystem = mount();
    let hello = filesystem.lookup("/hello.txt").unwrap();
    let mut buffer = [0u8; 64];
    let count = filesystem.read_file(hello, 0, &mut buffer).unwrap();
    assert_eq!(&buffer[..count], b"hello from ext2\n");
    let count = filesystem.read_file(hello, 6, &mut buffer).unwrap();
    assert_eq!(&buffer[..count], b"from ext2\n");
    assert_eq!(filesystem.read_file(hello, 100, &mut buffer).unwrap(), 0);
    assert_eq!(
        filesystem.read_file(2, 0, &mut buffer),
        Err(Ext2Error::NotFile)
    );

    let large = filesystem.lookup("/large.bin").unwrap();
    assert_eq!(filesystem.stat_inode(large).unwrap().size, 300 * 1024);
    for offset in [
        0usize,
        12 * 1024 - 1,
        12 * 1024,
        12 * 1024 - 8,
        268 * 1024 - 1,
        268 * 1024,
        268 * 1024 - 8,
        300 * 1024 - 1,
    ] {
        let mut buffer = [0u8; 32];
        let count = filesystem
            .read_file(large, offset as u64, &mut buffer)
            .unwrap();
        for index in 0..count {
            assert_eq!(buffer[index], pattern(offset + index), "offset {}", offset + index);
        }
    }
}

#[test]
fn lists_root_and_subdirectory_with_cursor() {
    let mut filesystem = mount();
    let mut names = Vec::new();
    let mut cursor = DirCursor::new();
    while let Some(entry) = filesystem.read_dir(2, &mut cursor).unwrap() {
        names.push(entry.name);
    }
    for expected in [".", "..", "lost+found", "hello.txt", "readme.txt", "sub"] {
        assert!(names.iter().any(|name| name == expected), "missing {}", expected);
    }
    let sub = filesystem.lookup("/sub").unwrap();
    let mut names = Vec::new();
    let mut cursor = DirCursor::new();
    while let Some(entry) = filesystem.read_dir(sub, &mut cursor).unwrap() {
        names.push(entry.name);
    }
    assert!(names.iter().any(|name| name == "."));
    assert!(names.iter().any(|name| name == ".."));
    assert!(names.iter().any(|name| name == "nested.txt"));
}

#[test]
fn rejects_corrupt_directory_rec_len() {
    let mut block = [0u8; 1024];
    block[0..4].copy_from_slice(&2u32.to_le_bytes());
    block[4..6].copy_from_slice(&12u16.to_le_bytes());
    block[6] = 4;
    block[7] = 2;
    block[8..12].copy_from_slice(b"abcd");
    let (entry, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(entry.inode, 2);
    assert_eq!(entry.name_len, 4);
    assert_eq!(next, 12);

    for rec_len in [0u16, 6, 10, 1040] {
        let mut bad = block;
        bad[4..6].copy_from_slice(&rec_len.to_le_bytes());
        assert_eq!(
            parse_dir_entry(&bad, 0, true),
            Err(Ext2Error::CorruptDirectory),
            "rec_len {}",
            rec_len
        );
    }

    let mut bad = block;
    bad[4..6].copy_from_slice(&12u16.to_le_bytes());
    bad[6] = 5;
    assert_eq!(
        parse_dir_entry(&bad, 0, true),
        Err(Ext2Error::CorruptDirectory)
    );

    let mut bad = block;
    bad[6] = 0;
    assert_eq!(
        parse_dir_entry(&bad, 0, true),
        Err(Ext2Error::CorruptDirectory)
    );
}

#[test]
fn maps_all_indirect_boundaries_for_each_block_size() {
    for pointers in [256u64, 512, 1024] {
        let singly_end = 12 + pointers;
        let doubly_end = singly_end + pointers * pointers;
        let triply_end = doubly_end + pointers * pointers * pointers;
        assert_eq!(block_path(0, pointers), BlockPath::Direct(0));
        assert_eq!(block_path(11, pointers), BlockPath::Direct(11));
        assert_eq!(block_path(12, pointers), BlockPath::Singly(0));
        assert_eq!(
            block_path(singly_end - 1, pointers),
            BlockPath::Singly(pointers - 1)
        );
        assert_eq!(
            block_path(singly_end, pointers),
            BlockPath::Doubly(0, 0)
        );
        assert_eq!(
            block_path(singly_end + pointers - 1, pointers),
            BlockPath::Doubly(0, pointers - 1)
        );
        assert_eq!(
            block_path(singly_end + pointers, pointers),
            BlockPath::Doubly(1, 0)
        );
        assert_eq!(
            block_path(doubly_end - 1, pointers),
            BlockPath::Doubly(pointers - 1, pointers - 1)
        );
        assert_eq!(block_path(doubly_end, pointers), BlockPath::Triply(0, 0, 0));
        assert_eq!(
            block_path(doubly_end + pointers * pointers - 1, pointers),
            BlockPath::Triply(0, pointers - 1, pointers - 1)
        );
        assert_eq!(
            block_path(triply_end - 1, pointers),
            BlockPath::Triply(pointers - 1, pointers - 1, pointers - 1)
        );
        assert_eq!(block_path(triply_end, pointers), BlockPath::OutOfRange);
    }
}

#[test]
fn reports_offending_unsupported_feature_bits() {
    let mut data = image();
    let superblock = 1024;
    let compat = u32::from_le_bytes(data[superblock + 92..superblock + 96].try_into().unwrap()) | 0x4;
    let incompat =
        u32::from_le_bytes(data[superblock + 96..superblock + 100].try_into().unwrap()) | 0x80;
    let ro_compat =
        u32::from_le_bytes(data[superblock + 100..superblock + 104].try_into().unwrap()) | 0x400;
    data[superblock + 92..superblock + 96].copy_from_slice(&compat.to_le_bytes());
    data[superblock + 96..superblock + 100].copy_from_slice(&incompat.to_le_bytes());
    data[superblock + 100..superblock + 104].copy_from_slice(&ro_compat.to_le_bytes());
    let error = match Ext2::mount(MemoryDevice::new(data)) {
        Err(error) => error,
        Ok(_) => panic!("unsupported features mounted"),
    };
    assert_eq!(
        error,
        Ext2Error::UnsupportedFeature {
            compat: 0x4,
            incompat: 0x80,
            ro_compat: 0x400
        }
    );
    let message = format!("{}", error);
    assert!(message.contains("compat=0x4"));
    assert!(message.contains("incompat=0x80"));
    assert!(message.contains("ro_compat=0x400"));
}

#[test]
fn block_cache_reuses_blocks() {
    let mut filesystem = mount();
    let node = filesystem.lookup("/hello.txt").unwrap();
    let mut buffer = [0u8; 64];
    filesystem.read_file(node, 0, &mut buffer).unwrap();
    let (hits_before, misses_before) = filesystem.cache_stats();
    filesystem.read_file(node, 0, &mut buffer).unwrap();
    let (hits_after, misses_after) = filesystem.cache_stats();
    assert!(hits_after > hits_before);
    assert_eq!(misses_after, misses_before);
}

#[test]
fn vfs_wraps_ext2_open_read_stat_seek_and_readdir() {
    let ext2 = mount();
    let mut vfs = Vfs::new(Box::new(ext2));
    let mut file = vfs.open("/hello.txt").unwrap();
    let mut buffer = [0u8; 32];
    let count = vfs.read(&mut file, &mut buffer).unwrap();
    assert_eq!(&buffer[..count], b"hello from ext2\n");
    let stat = vfs.stat("/hello.txt").unwrap();
    assert_eq!(stat.kind, FileType::Regular);
    assert_eq!(stat.size, 16);
    let offset = vfs.seek(&mut file, 0, 0).unwrap();
    assert_eq!(offset, 0);
    let count = vfs.read(&mut file, &mut buffer).unwrap();
    assert_eq!(&buffer[..count], b"hello from ext2\n");
    let mut cursor = DirCursor::new();
    let mut found = false;
    while let Some(entry) = vfs.readdir(2, &mut cursor).unwrap() {
        if entry.name == "hello.txt" {
            found = true;
        }
    }
    assert!(found);
    assert!(vfs.open("/missing").is_err());
}

fn superblock_bytes() -> [u8; 1024] {
    let data = image();
    let mut raw = [0u8; 1024];
    raw.copy_from_slice(&data[1024..2048]);
    raw
}

fn synthetic_superblock(
    blocks_count: u32,
    blocks_per_group: u32,
    inodes_count: u32,
    inodes_per_group: u32,
) -> Superblock {
    let mut raw = superblock_bytes();
    raw[0..4].copy_from_slice(&inodes_count.to_le_bytes());
    raw[4..8].copy_from_slice(&blocks_count.to_le_bytes());
    raw[32..36].copy_from_slice(&blocks_per_group.to_le_bytes());
    raw[40..44].copy_from_slice(&inodes_per_group.to_le_bytes());
    Superblock::parse(&raw).unwrap()
}

fn fresh_rw() -> Ext2<RamDisk> {
    Ext2::mount_rw_with_clock(RamDisk::new(image()), || 1_700_000_000).unwrap()
}

fn e2fsck(data: &[u8]) {
    let path = std::env::temp_dir().join(format!(
        "rinux-fsck-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, data).unwrap();
    let output = Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg(&path)
        .output()
        .expect("e2fsck must be installed");
    fs::remove_file(&path).ok();
    assert!(
        output.status.success(),
        "e2fsck failed with {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bitmap_sets_clears_and_finds_free_bits() {
    let mut bitmap = Bitmap::new(2);
    assert_eq!(bitmap.len_bits(), 16);
    assert_eq!(bitmap.count_free(16), 16);
    assert_eq!(bitmap.first_free(16, 0), Some(0));

    bitmap.set(0);
    bitmap.set(9);
    bitmap.set(15);
    assert!(bitmap.is_set(0));
    assert!(bitmap.is_set(9));
    assert!(bitmap.is_set(15));
    assert!(!bitmap.is_set(1));
    assert_eq!(bitmap.count_free(16), 13);
    assert_eq!(bitmap.first_free(16, 0), Some(1));
    assert_eq!(bitmap.first_free(16, 9), Some(10));
    assert_eq!(bitmap.first_free(16, 15), None);
    assert_eq!(bitmap.first_free(12, 0), Some(1));

    bitmap.clear(0);
    assert_eq!(bitmap.first_free(16, 0), Some(0));
    assert_eq!(bitmap.count_free(16), 14);

    // Bits outside the backing store are reported clear but never panic.
    assert!(!bitmap.is_set(1_000));
    bitmap.set(1_000);
    bitmap.clear(1_000);

    let round_trip = Bitmap::from_bytes(bitmap.as_bytes().to_vec());
    assert_eq!(round_trip, bitmap);
}

#[test]
fn group_math_clamps_the_last_group() {
    let superblock = synthetic_superblock(20_000, 8_192, 4_096, 2_048);
    assert_eq!(superblock.groups(), 3);
    assert_eq!(superblock.first_data_block(), 1);

    assert_eq!(layout::blocks_in_group(&superblock, 0), 8_192);
    assert_eq!(layout::blocks_in_group(&superblock, 1), 8_192);
    assert_eq!(layout::blocks_in_group(&superblock, 2), 3_615);
    assert_eq!(layout::blocks_in_group(&superblock, 3), 0);

    assert_eq!(layout::block_location(&superblock, 0), None);
    assert_eq!(layout::block_location(&superblock, 1), Some((0, 0)));
    assert_eq!(layout::block_location(&superblock, 8_192), Some((0, 8_191)));
    assert_eq!(layout::block_location(&superblock, 8_193), Some((1, 0)));
    assert_eq!(layout::block_location(&superblock, 20_000), None);
    assert_eq!(layout::block_number(&superblock, 0, 0), Some(1));
    assert_eq!(layout::block_number(&superblock, 2, 0), Some(16_385));
    assert_eq!(layout::block_number(&superblock, 2, 3_614), Some(19_999));
    assert_eq!(layout::block_number(&superblock, 2, 3_615), None);
    assert_eq!(layout::block_number(&superblock, 0, 8_192), None);

    assert_eq!(layout::inodes_in_group(&superblock, 0), 2_048);
    assert_eq!(layout::inodes_in_group(&superblock, 1), 2_048);
    assert_eq!(layout::inodes_in_group(&superblock, 2), 0);
    assert_eq!(layout::inode_location(&superblock, 0), None);
    assert_eq!(layout::inode_location(&superblock, 1), Some((0, 0)));
    assert_eq!(layout::inode_location(&superblock, 2_048), Some((0, 2_047)));
    assert_eq!(layout::inode_location(&superblock, 2_049), Some((1, 0)));
    assert_eq!(layout::inode_location(&superblock, 4_096), Some((1, 2_047)));
    assert_eq!(layout::inode_location(&superblock, 4_097), None);
    assert_eq!(layout::inode_number(&superblock, 0, 0), Some(1));
    assert_eq!(layout::inode_number(&superblock, 1, 0), Some(2_049));
    assert_eq!(layout::inode_number(&superblock, 1, 2_047), Some(4_096));
    assert_eq!(layout::inode_number(&superblock, 1, 2_048), None);

    assert_eq!(layout::first_inode_bit(&superblock, 0), 10);
    assert_eq!(layout::first_inode_bit(&superblock, 1), 0);
}

#[test]
fn read_only_mount_rejects_allocation_and_free() {
    let mut filesystem = Ext2::mount(RamDisk::new(image())).unwrap();
    assert!(filesystem.is_read_only());
    assert_eq!(filesystem.allocate_block(0), Err(Ext2Error::ReadOnly));
    assert_eq!(filesystem.allocate_inode(0, false), Err(Ext2Error::ReadOnly));
    assert_eq!(filesystem.free_block(1), Err(Ext2Error::ReadOnly));
    assert_eq!(filesystem.free_inode(11, false), Err(Ext2Error::ReadOnly));
    filesystem.sync().unwrap();
}

#[test]
fn ramdisk_allocates_and_frees_blocks_and_inodes() {
    let mut filesystem = fresh_rw();
    assert!(!filesystem.is_read_only());

    let free_blocks = filesystem.superblock().free_blocks_count();
    let free_inodes = filesystem.superblock().free_inodes_count();
    let descriptor = *filesystem.group_descriptor(0).unwrap();
    assert_eq!(descriptor.free_blocks_count as u32, free_blocks);

    let first = filesystem.allocate_block(0).unwrap();
    assert!(filesystem.is_block_allocated(first).unwrap());
    assert_eq!(filesystem.superblock().free_blocks_count(), free_blocks - 1);
    assert_eq!(
        filesystem.group_descriptor(0).unwrap().free_blocks_count as u32,
        free_blocks - 1
    );

    let second = filesystem.allocate_block(0).unwrap();
    assert_ne!(first, second);
    assert!(filesystem.is_block_allocated(second).unwrap());
    assert_eq!(filesystem.superblock().free_blocks_count(), free_blocks - 2);

    filesystem.free_block(first).unwrap();
    assert!(!filesystem.is_block_allocated(first).unwrap());
    assert_eq!(filesystem.superblock().free_blocks_count(), free_blocks - 1);

    filesystem.free_block(second).unwrap();
    assert_eq!(filesystem.superblock().free_blocks_count(), free_blocks);

    let inode = filesystem.allocate_inode(0, false).unwrap();
    assert!(filesystem.is_inode_allocated(inode).unwrap());
    assert_eq!(filesystem.superblock().free_inodes_count(), free_inodes - 1);
    assert_eq!(
        filesystem.group_descriptor(0).unwrap().free_inodes_count as u32,
        free_inodes - 1
    );
    assert_eq!(
        filesystem.group_descriptor(0).unwrap().used_dirs_count,
        descriptor.used_dirs_count
    );

    let directory = filesystem.allocate_inode(0, true).unwrap();
    assert_ne!(inode, directory);
    assert_eq!(
        filesystem.group_descriptor(0).unwrap().used_dirs_count,
        descriptor.used_dirs_count + 1
    );

    filesystem.free_inode(inode, false).unwrap();
    filesystem.free_inode(directory, true).unwrap();
    assert_eq!(filesystem.superblock().free_inodes_count(), free_inodes);
    assert_eq!(
        filesystem.group_descriptor(0).unwrap().used_dirs_count,
        descriptor.used_dirs_count
    );
    assert_eq!(
        filesystem.group_descriptor(0).unwrap().free_blocks_count,
        descriptor.free_blocks_count
    );
}

#[test]
fn allocation_round_trips_through_the_device() {
    let mut filesystem = fresh_rw();
    let block = filesystem.allocate_block(0).unwrap();
    let inode = filesystem.allocate_inode(0, false).unwrap();
    let free_blocks = filesystem.superblock().free_blocks_count();
    let free_inodes = filesystem.superblock().free_inodes_count();
    filesystem.sync().unwrap();
    let data = filesystem.into_device().into_data();

    let mut reopened = Ext2::mount_rw_with_clock(RamDisk::new(data), || 1_700_000_000).unwrap();
    assert_eq!(reopened.superblock().free_blocks_count(), free_blocks);
    assert_eq!(reopened.superblock().free_inodes_count(), free_inodes);
    assert!(reopened.is_block_allocated(block).unwrap());
    assert!(reopened.is_inode_allocated(inode).unwrap());

    reopened.free_block(block).unwrap();
    reopened.free_inode(inode, false).unwrap();
    reopened.sync().unwrap();
    let data = reopened.into_device().into_data();

    let mut reopened = Ext2::mount_rw_with_clock(RamDisk::new(data), || 1_700_000_000).unwrap();
    assert!(!reopened.is_block_allocated(block).unwrap());
    assert!(!reopened.is_inode_allocated(inode).unwrap());
}

#[test]
fn synced_allocator_state_passes_e2fsck() {
    let mut filesystem = fresh_rw();
    let blocks: Vec<u32> = (0..4)
        .map(|_| filesystem.allocate_block(0).unwrap())
        .collect();
    let inodes: Vec<u32> = (0..3)
        .map(|_| filesystem.allocate_inode(0, false).unwrap())
        .collect();
    for block in &blocks {
        filesystem.free_block(*block).unwrap();
    }
    for inode in &inodes {
        filesystem.free_inode(*inode, false).unwrap();
    }
    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    assert!(disk.flushes() > 0);
    e2fsck(disk.data());
}

#[test]
fn device_write_failures_are_reported() {
    let mut filesystem = fresh_rw();
    filesystem.device_mut().set_fail_writes(true);
    assert_eq!(filesystem.allocate_block(0), Err(Ext2Error::Device));
}

#[test]
fn writes_across_direct_singly_and_doubly_indirect_blocks() {
    let mut filesystem = fresh_rw();
    let node = filesystem.lookup("/hello.txt").unwrap();
    let data: Vec<u8> = (0..300 * 1024).map(pattern).collect();
    assert_eq!(filesystem.write_file(node, 0, &data).unwrap(), data.len());
    assert_eq!(filesystem.stat_inode(node).unwrap().size, data.len() as u64);

    for offset in [
        0usize,
        11 * 1024,
        12 * 1024,
        12 * 1024 + 256 * 1024 - 1,
        12 * 1024 + 256 * 1024,
        300 * 1024 - 64,
    ] {
        let mut buffer = [0u8; 64];
        let count = filesystem
            .read_file(node, offset as u64, &mut buffer)
            .unwrap();
        for index in 0..count {
            assert_eq!(
                buffer[index],
                data[offset + index],
                "offset {}",
                offset + index
            );
        }
    }

    let patch = vec![0xAB; 50 * 1024];
    let offset = 100 * 1024;
    assert_eq!(
        filesystem.write_file(node, offset as u64, &patch).unwrap(),
        patch.len()
    );
    let mut buffer = vec![0u8; patch.len()];
    let count = filesystem
        .read_file(node, offset as u64, &mut buffer)
        .unwrap();
    assert_eq!(&buffer[..count], &patch[..]);

    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn truncating_a_large_file_frees_indirect_trees() {
    let mut filesystem = fresh_rw();
    let node = filesystem.lookup("/hello.txt").unwrap();
    let data: Vec<u8> = (0..300 * 1024).map(pattern).collect();
    filesystem.write_file(node, 0, &data).unwrap();
    let free_before = filesystem.superblock().free_blocks_count();

    filesystem.truncate_inode(node, 1000).unwrap();
    assert_eq!(filesystem.stat_inode(node).unwrap().size, 1000);
    assert!(
        filesystem.superblock().free_blocks_count() > free_before + 250,
        "expected the indirect trees to be freed"
    );

    let mut buffer = [0u8; 64];
    let count = filesystem.read_file(node, 900, &mut buffer).unwrap();
    assert_eq!(count, 64);
    for index in 0..count {
        assert_eq!(buffer[index], data[900 + index]);
    }
    assert_eq!(filesystem.read_file(node, 1000, &mut buffer).unwrap(), 0);

    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn sparse_writes_leave_zero_holes() {
    let mut filesystem = fresh_rw();
    let node = filesystem.lookup("/hello.txt").unwrap();
    let old_size = filesystem.stat_inode(node).unwrap().size;
    let offset = 300 * 1024 + 500;
    let payload = b"sparse tail";
    assert_eq!(
        filesystem.write_file(node, offset, payload).unwrap(),
        payload.len()
    );
    assert_eq!(
        filesystem.stat_inode(node).unwrap().size,
        offset + payload.len() as u64
    );

    // The gap right after the old end reads as zeros.
    let mut buffer = [0u8; 64];
    let count = filesystem.read_file(node, old_size, &mut buffer).unwrap();
    assert_eq!(count, 64);
    assert!(buffer.iter().all(|byte| *byte == 0));

    // The bytes just before the payload are part of the hole.
    let count = filesystem
        .read_file(node, offset - 32, &mut buffer)
        .unwrap();
    assert_eq!(count, 32 + payload.len());
    assert!(buffer[..32].iter().all(|byte| *byte == 0));
    assert_eq!(&buffer[32..32 + payload.len()], payload);

    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn partial_write_commits_prefix_when_space_runs_out() {
    let data = build_minimal_image("1024", "128", "256");
    let mut filesystem = Ext2::mount_rw_with_clock(RamDisk::new(data), || 1_700_000_000).unwrap();
    let node = filesystem.lookup("/hello.txt").unwrap();
    let free = filesystem.superblock().free_blocks_count() as usize;
    let payload: Vec<u8> = (0..(free + 8) * 1024).map(pattern).collect();
    let written = filesystem.write_file(node, 0, &payload).unwrap();
    assert!(written > 0, "expected a partial write");
    assert!(written < payload.len(), "expected the write to stop early");
    assert_eq!(
        filesystem.stat_inode(node).unwrap().size,
        written as u64
    );

    let mut buffer = vec![0u8; written];
    let count = filesystem.read_file(node, 0, &mut buffer).unwrap();
    assert_eq!(count, written);
    assert_eq!(buffer, payload[..written]);

    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn truncate_shrinks_and_grows_with_zero_fill() {
    let mut filesystem = fresh_rw();
    let node = filesystem.lookup("/hello.txt").unwrap();
    let data = vec![0x5A; 5000];
    assert_eq!(filesystem.write_file(node, 0, &data).unwrap(), data.len());

    let free_before = filesystem.superblock().free_blocks_count();
    filesystem.truncate_inode(node, 1500).unwrap();
    assert_eq!(filesystem.stat_inode(node).unwrap().size, 1500);
    assert!(filesystem.superblock().free_blocks_count() > free_before);

    let mut buffer = [0u8; 64];
    let count = filesystem.read_file(node, 1440, &mut buffer).unwrap();
    assert_eq!(count, 60);
    assert!(buffer[..count].iter().all(|byte| *byte == 0x5A));
    assert_eq!(filesystem.read_file(node, 1500, &mut buffer).unwrap(), 0);

    filesystem.truncate_inode(node, 3000).unwrap();
    assert_eq!(filesystem.stat_inode(node).unwrap().size, 3000);
    let count = filesystem.read_file(node, 1500, &mut buffer).unwrap();
    assert_eq!(count, 64);
    assert!(buffer.iter().all(|byte| *byte == 0));
    let count = filesystem.read_file(node, 2990, &mut buffer).unwrap();
    assert_eq!(count, 10);
    assert!(buffer[..count].iter().all(|byte| *byte == 0));

    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn vfs_write_append_and_truncate_follow_open_semantics() {
    let mut vfs = Vfs::new(Box::new(fresh_rw()));
    let mut file = vfs.open("/hello.txt").unwrap();
    assert_eq!(vfs.write(&mut file, b"abc").unwrap(), 3);
    assert_eq!(file.offset, 3);

    let size = vfs.stat("/hello.txt").unwrap().size;
    let mut appending = vfs.open("/hello.txt").unwrap();
    appending.offset = 1;
    let (count, end) = vfs.append(&mut appending, b"XYZ").unwrap();
    assert_eq!(count, 3);
    assert_eq!(end, size + 3);
    assert_eq!(appending.offset, end);

    // O_TRUNC truncates before the descriptor starts using the file.
    vfs.truncate(file.node, 0).unwrap();
    assert_eq!(vfs.stat("/hello.txt").unwrap().size, 0);
    let (count, end) = vfs.append(&mut appending, b"fresh").unwrap();
    assert_eq!((count, end), (5, 5));
    let mut buffer = [0u8; 8];
    let count = vfs.read_at(file.node, 0, &mut buffer).unwrap();
    assert_eq!(&buffer[..count], b"fresh");
}

#[test]
fn directory_block_insert_splits_and_reuses_space() {
    let mut block = vec![0u8; 1024];
    init_empty_dir_block(&mut block);
    let (hole, end) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(hole.inode, 0);
    assert_eq!(end, 1024);

    assert!(insert_dir_entry(&mut block, b"alpha", 11, 1, true));
    let (entry, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(entry.inode, 11);
    assert_eq!(entry.file_type, 1);
    assert_eq!(entry.name_len, 5);
    assert_eq!(&block[entry.name_start..entry.name_start + 5], b"alpha");
    assert_eq!(next, 16);
    let (hole, end) = parse_dir_entry(&block, next, true).unwrap();
    assert_eq!(hole.inode, 0);
    assert_eq!(end, 1024);

    // The free hole left by the first insert is reused.
    assert!(insert_dir_entry(&mut block, b"beta", 12, 1, true));
    let (entry, next) = parse_dir_entry(&block, 16, true).unwrap();
    assert_eq!(entry.inode, 12);
    assert_eq!(&block[entry.name_start..entry.name_start + 4], b"beta");
    assert_eq!(next, 28);
    let (hole, end) = parse_dir_entry(&block, 28, true).unwrap();
    assert_eq!(hole.inode, 0);
    assert_eq!(end, 1024);

    // An active entry that still owns slack is split in place.
    let mut block = vec![0u8; 1024];
    block[0..4].copy_from_slice(&7u32.to_le_bytes());
    block[4..6].copy_from_slice(&1024u16.to_le_bytes());
    block[6] = 1;
    block[7] = 1;
    block[8] = b'a';
    assert!(insert_dir_entry(&mut block, b"b", 8, 1, true));
    let (first, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(first.inode, 7);
    assert_eq!(next, 12);
    let (second, next) = parse_dir_entry(&block, 12, true).unwrap();
    assert_eq!(second.inode, 8);
    assert_eq!(next, 24);
    let (hole, end) = parse_dir_entry(&block, 24, true).unwrap();
    assert_eq!(hole.inode, 0);
    assert_eq!(end, 1024);
}

#[test]
fn directory_block_insert_absorbs_small_remainders() {
    let mut block = vec![0u8; 1024];
    // Active entry "a" claims 28 bytes, followed by a free hole.
    block[0..4].copy_from_slice(&7u32.to_le_bytes());
    block[4..6].copy_from_slice(&28u16.to_le_bytes());
    block[6] = 1;
    block[7] = 1;
    block[8] = b'a';
    block[28..32].copy_from_slice(&0u32.to_le_bytes());
    block[32..34].copy_from_slice(&996u16.to_le_bytes());

    assert!(insert_dir_entry(&mut block, b"b", 8, 1, true));
    let (first, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(first.inode, 7);
    assert_eq!(next, 12);
    let (second, next) = parse_dir_entry(&block, 12, true).unwrap();
    assert_eq!(second.inode, 8);
    // 16 bytes remain, 12 are needed, so the 4-byte tail is absorbed.
    assert_eq!(next, 28);
    let (hole, end) = parse_dir_entry(&block, 28, true).unwrap();
    assert_eq!(hole.inode, 0);
    assert_eq!(end, 1024);

    // A free hole that fits exactly keeps its full length.
    let mut block = vec![0u8; 12];
    init_empty_dir_block(&mut block);
    assert!(insert_dir_entry(&mut block, b"x", 5, 1, true));
    let (entry, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(entry.inode, 5);
    assert_eq!(next, 12);
}

#[test]
fn directory_block_insert_without_filetype_uses_u16_name_length() {
    let mut block = vec![0u8; 1024];
    init_empty_dir_block(&mut block);
    assert!(insert_dir_entry(&mut block, b"name", 9, 0, false));
    let (entry, next) = parse_dir_entry(&block, 0, false).unwrap();
    assert_eq!(entry.inode, 9);
    assert_eq!(entry.name_len, 4);
    assert_eq!(u16::from_le_bytes([block[6], block[7]]), 4);
    assert_eq!(&block[8..12], b"name");
    assert_eq!(next, 12);
}

#[test]
fn directory_block_remove_merges_with_predecessor() {
    let mut block = vec![0u8; 1024];
    block[0..4].copy_from_slice(&1u32.to_le_bytes());
    block[4..6].copy_from_slice(&12u16.to_le_bytes());
    block[6] = 1;
    block[7] = 1;
    block[8] = b'a';
    block[12..16].copy_from_slice(&2u32.to_le_bytes());
    block[16..18].copy_from_slice(&12u16.to_le_bytes());
    block[18] = 1;
    block[19] = 1;
    block[20] = b'b';
    block[24..28].copy_from_slice(&0u32.to_le_bytes());
    block[28..30].copy_from_slice(&1000u16.to_le_bytes());

    remove_dir_entry(&mut block, 12, true).unwrap();
    let (first, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(first.inode, 1);
    assert_eq!(next, 24);
    let (hole, end) = parse_dir_entry(&block, 24, true).unwrap();
    assert_eq!(hole.inode, 0);
    assert_eq!(end, 1024);
}

#[test]
fn directory_block_remove_first_entry_leaves_a_hole() {
    let mut block = vec![0u8; 1024];
    block[0..4].copy_from_slice(&1u32.to_le_bytes());
    block[4..6].copy_from_slice(&12u16.to_le_bytes());
    block[6] = 1;
    block[7] = 1;
    block[8] = b'a';
    block[12..16].copy_from_slice(&0u32.to_le_bytes());
    block[16..18].copy_from_slice(&1012u16.to_le_bytes());

    remove_dir_entry(&mut block, 0, true).unwrap();
    let (hole, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(hole.inode, 0);
    assert_eq!(next, 12);
    assert_eq!(u16::from_le_bytes([block[4], block[5]]), 12);
    let (free, end) = parse_dir_entry(&block, 12, true).unwrap();
    assert_eq!(free.inode, 0);
    assert_eq!(end, 1024);
}

#[test]
fn directory_block_initialises_dot_and_dotdot() {
    let mut block = vec![0u8; 1024];
    init_dot_dotdot_block(&mut block, 2, 2, true);
    let (dot, next) = parse_dir_entry(&block, 0, true).unwrap();
    assert_eq!(dot.inode, 2);
    assert_eq!(dot.name_len, 1);
    assert_eq!(&block[dot.name_start..dot.name_start + 1], b".");
    assert_eq!(next, 12);
    let (dotdot, next) = parse_dir_entry(&block, 12, true).unwrap();
    assert_eq!(dotdot.inode, 2);
    assert_eq!(dotdot.name_len, 2);
    assert_eq!(&block[dotdot.name_start..dotdot.name_start + 2], b"..");
    assert_eq!(next, 1024);
}

#[test]
fn creates_files_with_o_creat_and_o_excl() {
    let mut filesystem = fresh_rw();
    let node = filesystem
        .open_flags("/created.txt", O_CREAT | O_RDWR, 0o640)
        .unwrap();
    let stat = filesystem.stat_inode(node).unwrap();
    assert_eq!(stat.size, 0);
    assert_eq!(stat.kind, FileType::Regular);
    assert_eq!(stat.mode & 0o777, 0o640);
    assert_eq!(filesystem.lookup("/created.txt").unwrap(), node);
    assert_eq!(filesystem.write_file(node, 0, b"created").unwrap(), 7);
    let mut buffer = [0u8; 16];
    let count = filesystem.read_file(node, 0, &mut buffer).unwrap();
    assert_eq!(&buffer[..count], b"created");

    // O_CREAT on an existing file just opens it without truncating.
    assert_eq!(
        filesystem
            .open_flags("/created.txt", O_CREAT | O_RDWR, 0o600)
            .unwrap(),
        node
    );
    assert_eq!(filesystem.stat_inode(node).unwrap().size, 7);
    // O_CREAT | O_EXCL on a missing path creates the file.
    let exclusive = filesystem
        .open_flags("/exclusive.txt", O_CREAT | O_EXCL | O_RDWR, 0o600)
        .unwrap();
    assert_eq!(filesystem.lookup("/exclusive.txt").unwrap(), exclusive);
    assert_eq!(
        filesystem.open_flags("/created.txt", O_CREAT | O_EXCL | O_RDWR, 0o600),
        Err(Ext2Error::AlreadyExists)
    );
    // A missing file without O_CREAT is still an error.
    assert_eq!(
        filesystem.open_flags("/missing.txt", O_RDWR, 0o600),
        Err(Ext2Error::NotFound)
    );
    // Directories cannot be opened for writing.
    assert_eq!(
        filesystem.open_flags("/sub", O_RDWR, 0o600),
        Err(Ext2Error::NotFile)
    );

    // O_TRUNC empties an existing regular file, even with O_CREAT set.
    let hello = filesystem.lookup("/hello.txt").unwrap();
    assert_eq!(
        filesystem
            .open_flags("/hello.txt", O_CREAT | O_RDWR | O_TRUNC, 0)
            .unwrap(),
        hello
    );
    assert_eq!(filesystem.stat_inode(hello).unwrap().size, 0);

    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn creating_many_files_grows_the_directory_across_blocks() {
    let mut filesystem = fresh_rw();
    let root = filesystem.lookup("/").unwrap();
    let initial_size = filesystem.stat_inode(root).unwrap().size;
    let mut created = Vec::new();
    for index in 0..40 {
        let name = format!("/file-{:02}-{}.txt", index, "x".repeat(180));
        let node = filesystem
            .open_flags(&name, O_CREAT | O_RDWR, 0o644)
            .unwrap();
        let payload = format!("payload-{}", index);
        assert_eq!(
            filesystem.write_file(node, 0, payload.as_bytes()).unwrap(),
            payload.len()
        );
        created.push((name, node, payload));
    }
    assert!(
        filesystem.stat_inode(root).unwrap().size > initial_size + 3 * 1024,
        "directory should span several blocks"
    );
    for (name, node, payload) in &created {
        assert_eq!(filesystem.lookup(name).unwrap(), *node);
        let mut buffer = [0u8; 32];
        let count = filesystem.read_file(*node, 0, &mut buffer).unwrap();
        assert_eq!(&buffer[..count], payload.as_bytes());
    }

    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn vfs_open_with_flags_creates_and_truncates() {
    let mut vfs = Vfs::new(Box::new(fresh_rw()));
    let mut file = vfs
        .open_with_flags("/vfs-created.txt", O_CREAT | O_RDWR, 0o644)
        .unwrap();
    assert_eq!(file.flags & 0o3, O_RDWR);
    assert_eq!(vfs.write(&mut file, b"hello").unwrap(), 5);
    assert_eq!(vfs.stat("/vfs-created.txt").unwrap().size, 5);

    assert!(matches!(
        vfs.open_with_flags("/vfs-created.txt", O_CREAT | O_EXCL | O_RDWR, 0o644),
        Err(VfsError::AlreadyExists)
    ));
    let existing = vfs
        .open_with_flags("/vfs-created.txt", O_CREAT | O_RDWR, 0o644)
        .unwrap();
    assert_eq!(existing.node, file.node);

    let truncated = vfs
        .open_with_flags("/vfs-created.txt", O_RDWR | O_TRUNC, 0)
        .unwrap();
    assert_eq!(vfs.stat_node(truncated.node).unwrap().size, 0);
    assert!(matches!(
        vfs.open_with_flags("/sub", O_RDWR, 0),
        Err(VfsError::NotFile)
    ));
}

#[test]
fn writes_and_truncates_on_four_kib_blocks() {
    let data = build_minimal_image("4096", "256", "8192");
    let mut filesystem = Ext2::mount_rw_with_clock(RamDisk::new(data), || 1_700_000_000).unwrap();
    let node = filesystem.lookup("/hello.txt").unwrap();
    let payload: Vec<u8> = (0..200 * 1024).map(pattern).collect();
    assert_eq!(
        filesystem.write_file(node, 0, &payload).unwrap(),
        payload.len()
    );
    assert_eq!(
        filesystem.stat_inode(node).unwrap().size,
        payload.len() as u64
    );

    let mut buffer = [0u8; 128];
    let count = filesystem
        .read_file(node, 200 * 1024 - 64, &mut buffer)
        .unwrap();
    assert_eq!(count, 64);
    for index in 0..count {
        assert_eq!(buffer[index], payload[200 * 1024 - 64 + index]);
    }

    filesystem.truncate_inode(node, 50 * 1024).unwrap();
    assert_eq!(
        filesystem.stat_inode(node).unwrap().size,
        50 * 1024
    );
    let created = filesystem
        .open_flags("/created-256.txt", O_CREAT | O_RDWR, 0o644)
        .unwrap();
    assert_eq!(
        filesystem.write_file(created, 0, b"created").unwrap(),
        7
    );
    assert_eq!(filesystem.lookup("/created-256.txt").unwrap(), created);
    filesystem.sync().unwrap();
    let disk = filesystem.into_device();
    e2fsck(disk.data());
}

#[test]
fn allocates_on_four_kib_blocks_and_256_byte_inodes() {
    let data = build_minimal_image("4096", "256", "8192");
    let mut filesystem = Ext2::mount_rw_with_clock(RamDisk::new(data), || 1_700_000_000).unwrap();
    assert_eq!(filesystem.superblock().block_size(), 4096);
    assert_eq!(filesystem.superblock().inode_size(), 256);

    let block = filesystem.allocate_block(0).unwrap();
    let inode = filesystem.allocate_inode(0, true).unwrap();
    assert!(filesystem.is_block_allocated(block).unwrap());
    assert!(filesystem.is_inode_allocated(inode).unwrap());
    filesystem.free_block(block).unwrap();
    filesystem.free_inode(inode, true).unwrap();
    filesystem.sync().unwrap();

    let disk = filesystem.into_device();
    e2fsck(disk.data());
}
