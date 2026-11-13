use hosttest::fs::ext2::{
    block_path, parse_dir_entry, BlockDevice, BlockPath, Ext2, Ext2Error,
};
use hosttest::fs::vfs::{DirCursor, FileType, Vfs};
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
