//! Leitura de SSTables LevelDB, sem abrir nem modificar a base do Chrome.
//! Formato: google/leveldb doc/table_format.md e table/block_builder.cc.

use std::collections::HashSet;

const MAGIC: [u8; 8] = 0xdb4775248b80fb57u64.to_le_bytes();
const FOOTER_BYTES: usize = 48;
const MAX_BLOCK_BYTES: usize = 8 * 1024 * 1024;
const MAX_KEY_BYTES: usize = 2048;

pub(super) struct Budget {
    pub(super) bytes: usize,
    pub(super) entries: usize,
}

pub(super) type Entries = Vec<(Vec<u8>, Vec<u8>)>;

pub(super) fn is_table(bytes: &[u8]) -> bool {
    bytes.len() >= FOOTER_BYTES && bytes.ends_with(&MAGIC)
}

fn varint(bytes: &[u8], cursor: &mut usize) -> Option<usize> {
    usize::try_from(super::read_varint(bytes, cursor)?).ok()
}

fn handle(bytes: &[u8], cursor: &mut usize) -> Option<(usize, usize)> {
    Some((varint(bytes, cursor)?, varint(bytes, cursor)?))
}

/// Primeiro confere CRC e tamanho declarado, depois descomprime para uma
/// alocacao limitada. O budget e partilhado por todos os ficheiros do perfil.
fn block(file: &[u8], at: usize, size: usize, budget: &mut Budget) -> Option<Vec<u8>> {
    let end = at.checked_add(size)?;
    let trailer_end = end.checked_add(5)?;
    if size > MAX_BLOCK_BYTES || trailer_end > file.len().checked_sub(FOOTER_BYTES)? {
        return None;
    }
    let payload = file.get(at..end)?;
    let kind = *file.get(end)?;
    let stored_crc = u32::from_le_bytes(file.get(end + 1..trailer_end)?.try_into().ok()?);
    if masked_crc32c(file.get(at..end + 1)?) != stored_crc {
        return None;
    }
    let decoded_size = match kind {
        0 => payload.len(),
        1 => snap::raw::decompress_len(payload).ok()?,
        _ => return None,
    };
    if decoded_size > MAX_BLOCK_BYTES || decoded_size > budget.bytes {
        return None;
    }
    budget.bytes -= decoded_size;
    match kind {
        0 => Some(payload.to_vec()),
        1 => {
            let mut decoded = vec![0; decoded_size];
            let written = snap::raw::Decoder::new()
                .decompress(payload, &mut decoded)
                .ok()?;
            (written == decoded_size).then_some(decoded)
        }
        _ => None,
    }
}

fn entries(block: &[u8], budget: &mut Budget) -> Option<Entries> {
    let count_at = block.len().checked_sub(4)?;
    let restarts = u32::from_le_bytes(block.get(count_at..)?.try_into().ok()?) as usize;
    let trailer_bytes = restarts.checked_add(1)?.checked_mul(4)?;
    let end = block.len().checked_sub(trailer_bytes)?;
    if restarts == 0 {
        return None;
    }
    let mut prior = 0;
    let restart_bytes = block.get(end..count_at)?;
    let (chunks, rest) = restart_bytes.as_chunks::<4>();
    if !rest.is_empty() || chunks.len() != restarts {
        return None;
    }
    for (ordinal, bytes) in chunks.iter().enumerate() {
        let at = u32::from_le_bytes(*bytes) as usize;
        if (ordinal == 0 && at != 0) || at < prior || at > end {
            return None;
        }
        prior = at;
    }
    let data = block.get(..end)?;
    let mut cursor = 0;
    let mut key = Vec::new();
    let mut out = Vec::new();
    while cursor < end {
        if budget.entries == 0 {
            return None;
        }
        let shared = varint(data, &mut cursor)?;
        let unshared = varint(data, &mut cursor)?;
        let value_size = varint(data, &mut cursor)?;
        if shared > key.len() || shared.checked_add(unshared)? > MAX_KEY_BYTES {
            return None;
        }
        let key_end = cursor.checked_add(unshared)?;
        let value_end = key_end.checked_add(value_size)?;
        let delta = data.get(cursor..key_end)?;
        let value = data.get(key_end..value_end)?;
        key.truncate(shared);
        key.extend_from_slice(delta);
        out.push((key.clone(), value.to_vec()));
        budget.entries -= 1;
        cursor = value_end;
    }
    Some(out)
}

/// Um ficheiro corrompido ou que excede limites nao entrega registos parciais.
pub(super) fn read_table(file: &[u8], budget: &mut Budget) -> Option<Entries> {
    if !is_table(file) {
        return None;
    }
    let footer = file.get(file.len() - FOOTER_BYTES..file.len() - 8)?;
    let mut cursor = 0;
    let _meta = handle(footer, &mut cursor)?;
    let (index_at, index_size) = handle(footer, &mut cursor)?;
    let index = block(file, index_at, index_size, budget)?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (_, value) in entries(&index, budget)? {
        let mut cursor = 0;
        let (at, size) = handle(&value, &mut cursor)?;
        if cursor != value.len() || !seen.insert((at, size)) || at >= index_at {
            return None;
        }
        let data = block(file, at, size, budget)?;
        out.extend(entries(&data, budget)?);
    }
    Some(out)
}

const fn crc32c_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 256 {
        let mut crc = byte as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82f63b78
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[byte] = crc;
        byte += 1;
    }
    table
}

const CRC32C_TABLE: [u32; 256] = crc32c_table();

fn masked_crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc = CRC32C_TABLE[((crc as u8) ^ byte) as usize] ^ (crc >> 8);
    }
    (!crc).rotate_right(15).wrapping_add(0xa282ead8)
}

#[cfg(test)]
pub(super) fn fixture_table(entries_in: &[(Vec<u8>, Vec<u8>)], compress: bool) -> Vec<u8> {
    fn make_block(items: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut prior: &[u8] = &[];
        for (key, value) in items {
            let shared = key.iter().zip(prior).take_while(|(a, b)| a == b).count();
            bytes.extend(super::tests::varint(shared as u64));
            bytes.extend(super::tests::varint((key.len() - shared) as u64));
            bytes.extend(super::tests::varint(value.len() as u64));
            bytes.extend(&key[shared..]);
            bytes.extend(value);
            prior = key;
        }
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes
    }
    fn append(file: &mut Vec<u8>, plain: &[u8], compress: bool) -> (usize, usize) {
        let at = file.len();
        let encoded = if compress {
            snap::raw::Encoder::new().compress_vec(plain).unwrap()
        } else {
            plain.to_vec()
        };
        let size = encoded.len();
        file.extend(encoded);
        file.push(u8::from(compress));
        let crc = masked_crc32c(&file[at..]);
        file.extend(crc.to_le_bytes());
        (at, size)
    }
    let mut file = Vec::new();
    let (data_at, data_size) = append(&mut file, &make_block(entries_in), compress);
    let handle = [
        super::tests::varint(data_at as u64),
        super::tests::varint(data_size as u64),
    ]
    .concat();
    let (index_at, index_size) = append(
        &mut file,
        &make_block(&[(b"index".to_vec(), handle)]),
        false,
    );
    let mut footer = vec![0, 0]; // Metaindex vazio; nao e preciso para ler dados.
    footer.extend(super::tests::varint(index_at as u64));
    footer.extend(super::tests::varint(index_size as u64));
    footer.resize(40, 0);
    footer.extend(MAGIC);
    file.extend(footer);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> Budget {
        Budget {
            bytes: super::super::MAX_PROFILE_BYTES,
            entries: 100_000,
        }
    }

    #[test]
    fn chrome_group_sstable_rejects_corruption_and_out_of_bounds_handles() {
        let expected = vec![
            (b"prefix-key-a".to_vec(), b"value-a".to_vec()),
            (b"prefix-key-b".to_vec(), b"value-b".to_vec()),
        ];
        let file = fixture_table(&expected, true);
        assert_eq!(read_table(&file, &mut budget()), Some(expected));
        let mut corrupt = file.clone();
        corrupt[0] ^= 1;
        assert!(read_table(&corrupt, &mut budget()).is_none());
        let mut bad_offset = file.clone();
        let footer_at = bad_offset.len() - FOOTER_BYTES;
        bad_offset[footer_at + 2] = 0x7f;
        assert!(read_table(&bad_offset, &mut budget()).is_none());
        assert!(
            read_table(
                &file,
                &mut Budget {
                    bytes: 1,
                    entries: 100
                }
            )
            .is_none()
        );
        assert!(
            read_table(
                &file,
                &mut Budget {
                    bytes: MAX_BLOCK_BYTES,
                    entries: 1
                }
            )
            .is_none()
        );
    }

    #[test]
    fn chrome_group_snappy_checks_declared_size_before_decompression() {
        let plain = vec![0; MAX_BLOCK_BYTES + 1];
        let encoded = snap::raw::Encoder::new().compress_vec(&plain).unwrap();
        let mut file = encoded.clone();
        file.push(1);
        file.extend(masked_crc32c(&file).to_le_bytes());
        file.resize(file.len() + FOOTER_BYTES, 0);
        assert!(block(&file, 0, encoded.len(), &mut budget()).is_none());
    }
}
