//! 只读 zip 的中央目录，找到条目在远程文件里的位置，不下载整个包。

use super::{remote::Source, *};
use bytes::Bytes;

const EOCD_SIG: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

/// EOCD 22 字节 + 最长 64K 注释
pub const TAIL_LEN: u64 = 22 + 0xFFFF + 20;

/// 存储方式
pub const STORED: u16 = 0;
pub const DEFLATED: u16 = 8;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub compressed_size: u64,
    pub size: u64,
    local_header_offset: u64,
}

impl Entry {
    /// 条目数据在整个 zip 里的起始偏移（跳过 local header）
    pub async fn data_offset(&self, src: &dyn Source) -> Result<u64, BotError> {
        let header = src.read(self.local_header_offset, 30).await?;
        if le32(&header, 0) != LOCAL_SIG {
            return Err(bad_zip("local header 签名不对"));
        }
        let name_len = le16(&header, 26) as u64;
        let extra_len = le16(&header, 28) as u64;
        Ok(self.local_header_offset + 30 + name_len + extra_len)
    }
}

fn bad_zip(why: &str) -> BotError {
    BotError::Custom(format!("不是有效的 zip 包：{why}"))
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// 读出中央目录里的所有条目。`tail` 是文件最后一段（从 `size - tail.len()` 开始）
pub async fn entries(src: &dyn Source, tail: &Bytes) -> Result<Vec<Entry>, BotError> {
    let tail_start = src.size() - tail.len() as u64;
    // 从后往前找 EOCD，跳过注释里碰巧出现的签名：注释长度要正好到文件尾
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| le32(tail, i) == EOCD_SIG && i + 22 + le16(tail, i + 20) as usize == tail.len())
        .ok_or_else(|| bad_zip("找不到目录结尾"))?;
    let mut count = le16(tail, eocd + 10) as u64;
    let mut cd_size = le32(tail, eocd + 12) as u64;
    let mut cd_offset = le32(tail, eocd + 16) as u64;

    // ZIP64：EOCD 前面紧挨着 20 字节的 locator
    if eocd >= 20 && le32(tail, eocd - 20) == ZIP64_LOCATOR_SIG {
        let zip64_offset = le64(tail, eocd - 20 + 8);
        let record = src.read(zip64_offset, 56).await?;
        if le32(&record, 0) != ZIP64_EOCD_SIG {
            return Err(bad_zip("ZIP64 目录结尾签名不对"));
        }
        count = le64(&record, 32);
        cd_size = le64(&record, 40);
        cd_offset = le64(&record, 48);
    }
    if cd_offset + cd_size > src.size() {
        return Err(bad_zip("目录位置超出文件"));
    }
    // 目录不会大到离谱，防止恶意链接撑爆内存
    if cd_size > 64 << 20 {
        return Err(bad_zip("目录过大"));
    }

    let cd = if cd_offset >= tail_start {
        let start = (cd_offset - tail_start) as usize;
        tail.slice(start..start + cd_size as usize)
    } else {
        src.read(cd_offset, cd_size).await?
    };
    parse_central_directory(&cd, count)
}

fn parse_central_directory(cd: &[u8], count: u64) -> Result<Vec<Entry>, BotError> {
    let mut entries = Vec::with_capacity(count.min(4096) as usize);
    let mut at = 0;
    while at + 46 <= cd.len() && le32(cd, at) == CENTRAL_SIG {
        let method = le16(cd, at + 10);
        let mut compressed_size = le32(cd, at + 20) as u64;
        let mut size = le32(cd, at + 24) as u64;
        let name_len = le16(cd, at + 28) as usize;
        let extra_len = le16(cd, at + 30) as usize;
        let comment_len = le16(cd, at + 32) as usize;
        let mut local_header_offset = le32(cd, at + 42) as u64;
        let name_end = at + 46 + name_len;
        let extra_end = name_end + extra_len;
        if extra_end + comment_len > cd.len() {
            return Err(bad_zip("目录条目被截断"));
        }
        let name = String::from_utf8_lossy(&cd[at + 46..name_end]).into_owned();

        // ZIP64 extra：只按顺序列出 32 位字段里填了 0xFFFFFFFF 的那几个
        let mut extra = &cd[name_end..extra_end];
        while extra.len() >= 4 {
            let id = le16(extra, 0);
            let len = (le16(extra, 2) as usize).min(extra.len() - 4);
            let mut field = &extra[4..4 + len];
            if id == 0x0001 {
                for value in [&mut size, &mut compressed_size, &mut local_header_offset] {
                    if *value == 0xFFFF_FFFF && field.len() >= 8 {
                        *value = le64(field, 0);
                        field = &field[8..];
                    }
                }
            }
            extra = &extra[4 + len..];
        }

        entries.push(Entry {
            name,
            method,
            compressed_size,
            size,
            local_header_offset,
        });
        at = extra_end + comment_len;
    }
    if entries.is_empty() {
        return Err(bad_zip("目录是空的"));
    }
    Ok(entries)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::funcs::command::dump::remote::Memory;

    /// 拼一个最简单的 zip：所有条目都按 STORED 存放
    pub fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&0u32.to_le_bytes()); // crc，解析时不看
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&3u16.to_le_bytes()); // 带一点 extra，验证跳过
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&[0, 0, 0]);
            out.extend_from_slice(data);

            central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[tokio::test]
    async fn finds_entries_and_their_data() {
        let zip = Bytes::from(build_zip(&[
            ("META-INF/x", b"hello"),
            ("payload.bin", b"CrAU-data"),
        ]));
        let src = Memory(zip.clone());
        // 末尾那段只给一部分，目录要再读一次
        let tail = zip.slice(zip.len() - 30..);
        let list = entries(&src, &tail).await.unwrap();
        assert_eq!(list.len(), 2);
        let payload = list.iter().find(|e| e.name == "payload.bin").unwrap();
        assert_eq!(payload.method, STORED);
        assert_eq!(payload.size, 9);
        let at = payload.data_offset(&src).await.unwrap();
        assert_eq!(&zip[at as usize..at as usize + 9], b"CrAU-data");
    }

    #[tokio::test]
    async fn rejects_non_zip() {
        let data = Bytes::from_static(&[0u8; 100]);
        let src = Memory(data.clone());
        assert!(entries(&src, &data).await.is_err());
    }
}
