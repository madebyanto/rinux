use hosttest::ata::{
    chunk_len, parse_identify, validate_request, AtaError, MAX_LBA28, MAX_SECTORS_PER_COMMAND,
    SECTOR_SIZE,
};

fn set_word(block: &mut [u8; 512], index: usize, value: u16) {
    block[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
}

fn set_model(block: &mut [u8; 512], text: &[u8]) {
    let mut model = [b' '; 40];
    let length = text.len().min(model.len());
    model[..length].copy_from_slice(&text[..length]);
    for index in 0..20 {
        let word = ((model[index * 2] as u16) << 8) | model[index * 2 + 1] as u16;
        set_word(block, 27 + index, word);
    }
}

fn identify_block() -> [u8; 512] {
    let mut block = [0u8; 512];
    set_word(&mut block, 0, 0x0040);
    set_word(&mut block, 60, 0x1234);
    set_word(&mut block, 61, 0x0001);
    set_word(&mut block, 83, 1 << 10);
    set_model(&mut block, b"QEMU HARDDISK");
    block
}

#[test]
fn parses_identify_sector_and_model() {
    let info = parse_identify(&identify_block()).unwrap();
    assert_eq!(info.sectors, 0x0001_1234);
    assert!(info.lba48);
    assert_eq!(info.model(), "QEMU HARDDISK");
}

#[test]
fn rejects_atapi_and_invalid_models() {
    let mut block = identify_block();
    set_word(&mut block, 0, 0x8000);
    assert_eq!(parse_identify(&block), Err(AtaError::IdentifyFailed));

    let mut block = identify_block();
    set_model(&mut block, &[0xFF; 40]);
    let info = parse_identify(&block).unwrap();
    assert_eq!(info.model(), "unknown");
}

#[test]
fn validates_lba28_boundaries_and_buffers() {
    assert_eq!(validate_request(0, 1, 512), Ok(512));
    assert_eq!(validate_request(0, 256, 256 * 512), Ok(256 * 512));
    assert_eq!(validate_request(0, 300, 300 * 512), Ok(300 * 512));
    assert_eq!(validate_request(0, 1, 1024), Ok(512));
    assert_eq!(validate_request(0, 0, 0), Err(AtaError::InvalidCount));
    assert_eq!(
        validate_request(MAX_LBA28, 1, 512),
        Err(AtaError::LbaOutOfRange)
    );
    assert_eq!(
        validate_request(MAX_LBA28 - 1, 2, 1024),
        Err(AtaError::LbaOutOfRange)
    );
    assert_eq!(
        validate_request(0, 1, 511),
        Err(AtaError::BufferTooSmall {
            needed: SECTOR_SIZE,
            provided: 511
        })
    );
}

#[test]
fn splits_transfers_into_ata_command_chunks() {
    assert_eq!(chunk_len(0), 0);
    assert_eq!(chunk_len(1), 1);
    assert_eq!(chunk_len(255), 255);
    assert_eq!(chunk_len(256), MAX_SECTORS_PER_COMMAND);
    assert_eq!(chunk_len(257), MAX_SECTORS_PER_COMMAND);
    assert_eq!(chunk_len(512), MAX_SECTORS_PER_COMMAND);
    assert_eq!(chunk_len(65536), MAX_SECTORS_PER_COMMAND);
}
