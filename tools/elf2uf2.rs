//! Project-local ELF32 -> RP2350 Secure ARM UF2 packer; no external crates.
//! Only accepts flash-linked little-endian ARM ELF images within this board's 4 MiB.
use std::{collections::BTreeMap, env, fs};
fn u16_at(b: &[u8], p: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        b.get(p..p + 2).ok_or("truncated ELF")?.try_into().unwrap(),
    ))
}
fn u32_at(b: &[u8], p: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        b.get(p..p + 4).ok_or("truncated ELF")?.try_into().unwrap(),
    ))
}
fn convert(elf: &[u8]) -> Result<Vec<u8>, String> {
    if elf.get(..7) != Some(&b"\x7fELF\x01\x01\x01"[..]) || u16_at(elf, 18)? != 40 {
        return Err("expected little-endian ELF32 ARM".into());
    }
    let offset = u32_at(elf, 28)? as usize;
    let entry_size = u16_at(elf, 42)? as usize;
    let count = u16_at(elf, 44)? as usize;
    if entry_size < 32 {
        return Err("invalid program header size".into());
    }
    let mut pages: BTreeMap<u32, [u8; 256]> = BTreeMap::new();
    for n in 0..count {
        let p = offset + n * entry_size;
        if u32_at(elf, p)? != 1 {
            continue;
        } // PT_LOAD
        let file_offset = u32_at(elf, p + 4)? as usize;
        let addr = u32_at(elf, p + 12)?; // physical/load address, including RAM .data LMA
        let size = u32_at(elf, p + 16)? as usize;
        if size == 0 {
            continue;
        } // BSS has no file bytes
        if addr < 0x10000000 || addr as u64 + size as u64 > 0x10400000 {
            return Err(format!("load segment outside board flash: {addr:08x}"));
        }
        let data = elf
            .get(file_offset..file_offset + size)
            .ok_or("truncated segment")?;
        for (i, &byte) in data.iter().enumerate() {
            let address = addr + i as u32;
            pages.entry(address & !255).or_insert([0xff; 256])[(address & 255) as usize] = byte;
        }
    }
    if !pages.contains_key(&0x10000000) {
        return Err("missing flash vector table".into());
    }
    let count = pages.len();
    let mut output = Vec::with_capacity(count * 512);
    for (n, (address, data)) in pages.into_iter().enumerate() {
        let mut block = [0u8; 512];
        let header = [
            0x0a324655u32,
            0x9e5d5157,
            0x2000,
            address,
            256,
            n as u32,
            count as u32,
            0xe48bff59,
        ];
        for (i, word) in header.iter().enumerate() {
            block[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        block[32..288].copy_from_slice(&data);
        block[508..512].copy_from_slice(&0x0ab16f30u32.to_le_bytes());
        output.extend_from_slice(&block);
    }
    Ok(output)
}
fn main() -> Result<(), String> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err("usage: elf2uf2 INPUT.elf OUTPUT.uf2".into());
    }
    let uf2 = convert(&fs::read(&args[1]).map_err(|e| e.to_string())?)?;
    fs::write(&args[2], &uf2).map_err(|e| e.to_string())?;
    println!(
        "{}: {} UF2 blocks ({} bytes)",
        args[2],
        uf2.len() / 512,
        uf2.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_elf() {
        assert!(convert(b"bad").is_err());
    }
    #[test]
    fn packs_load_address_and_partial_page() {
        let mut elf = vec![0u8; 0x200];
        elf[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
        elf[18..20].copy_from_slice(&40u16.to_le_bytes());
        elf[28..32].copy_from_slice(&52u32.to_le_bytes());
        elf[42..44].copy_from_slice(&32u16.to_le_bytes());
        elf[44..46].copy_from_slice(&1u16.to_le_bytes());
        for (offset, value) in [
            (52, 1u32),
            (56, 0x100),
            (60, 0x20000000),
            (64, 0x10000000),
            (68, 3),
        ] {
            elf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        elf[0x100..0x103].copy_from_slice(&[1, 2, 3]);
        let uf2 = convert(&elf).unwrap();
        assert_eq!(uf2.len(), 512);
        assert_eq!(u32_at(&uf2, 12).unwrap(), 0x10000000);
        assert_eq!(u32_at(&uf2, 28).unwrap(), 0xe48bff59);
        assert_eq!(&uf2[32..36], &[1, 2, 3, 255]);
        elf[64..68].copy_from_slice(&0x20000000u32.to_le_bytes());
        assert!(convert(&elf).is_err());
    }
}
