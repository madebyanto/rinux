use hosttest::fs::mbr::{absolute_lba, partition, MbrError, Partition};

fn block_with(start: u32, sectors: u32, kind: u8) -> [u8; 512] {
    let mut block = [0u8; 512];
    block[510] = 0x55;
    block[511] = 0xAA;
    let offset = 446;
    block[offset + 4] = kind;
    block[offset + 8..offset + 12].copy_from_slice(&start.to_le_bytes());
    block[offset + 12..offset + 16].copy_from_slice(&sectors.to_le_bytes());
    block
}

#[test]
fn parses_a_type_83_partition() {
    let block = block_with(2048, 24576, 0x83);
    assert_eq!(
        partition(&block),
        Ok(Partition {
            start_lba: 2048,
            sectors: 24576
        })
    );
}

#[test]
fn rejects_missing_signature_and_missing_partition() {
    let mut block = block_with(2048, 24576, 0x83);
    block[510] = 0;
    assert_eq!(partition(&block), Err(MbrError::Signature));
    let block = block_with(2048, 24576, 0x82);
    assert_eq!(partition(&block), Err(MbrError::NoPartition));
}

#[test]
fn maps_partition_offsets_to_absolute_lba() {
    assert_eq!(absolute_lba(2048, 0), Ok(2048));
    assert_eq!(absolute_lba(2048, 1024), Ok(2050));
    assert_eq!(absolute_lba(2048, 1536), Ok(2051));
    assert_eq!(absolute_lba(2048, 1), Err(MbrError::Misaligned));
    assert_eq!(
        absolute_lba(u32::MAX, 512),
        Err(MbrError::OutOfRange)
    );
}
