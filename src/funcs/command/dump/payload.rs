//! A/B 卡刷包里的 payload.bin（update_engine 格式）：读 manifest，
//! 只拉目标分区那些 operation 的数据，解压后按块写进镜像文件。

use super::{remote::Source, *};
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use prost::Message as _;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    os::unix::fs::FileExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const MAGIC: &[u8; 4] = b"CrAU";
/// manifest 的合理上限，防止恶意链接撑爆内存
const MAX_MANIFEST: u64 = 64 << 20;
/// 一次 Range 请求最多合并这么多相邻数据
const BATCH_BYTES: u64 = 8 << 20;
/// 同时在途的 Range 请求数
const CONCURRENCY: usize = 8;

// update_metadata.proto 里用到的部分（proto2，全部按 optional 处理）
#[derive(Clone, PartialEq, prost::Message)]
pub struct DeltaArchiveManifest {
    #[prost(uint32, optional, tag = "3")]
    pub block_size: Option<u32>,
    #[prost(uint32, optional, tag = "12")]
    pub minor_version: Option<u32>,
    #[prost(message, repeated, tag = "13")]
    pub partitions: Vec<PartitionUpdate>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PartitionUpdate {
    #[prost(string, optional, tag = "1")]
    pub partition_name: Option<String>,
    #[prost(message, optional, tag = "7")]
    pub new_partition_info: Option<PartitionInfo>,
    #[prost(message, repeated, tag = "8")]
    pub operations: Vec<InstallOperation>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PartitionInfo {
    #[prost(uint64, optional, tag = "1")]
    pub size: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub hash: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct InstallOperation {
    #[prost(int32, optional, tag = "1")]
    pub r#type: Option<i32>,
    #[prost(uint64, optional, tag = "2")]
    pub data_offset: Option<u64>,
    #[prost(uint64, optional, tag = "3")]
    pub data_length: Option<u64>,
    #[prost(message, repeated, tag = "6")]
    pub dst_extents: Vec<Extent>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Extent {
    #[prost(uint64, optional, tag = "1")]
    pub start_block: Option<u64>,
    #[prost(uint64, optional, tag = "2")]
    pub num_blocks: Option<u64>,
}

// InstallOperation.Type
const REPLACE: i32 = 0;
const REPLACE_BZ: i32 = 1;
const ZERO: i32 = 6;
const DISCARD: i32 = 7;
const REPLACE_XZ: i32 = 8;
const ZSTD: i32 = 14;

impl PartitionUpdate {
    pub fn name(&self) -> &str {
        self.partition_name.as_deref().unwrap_or_default()
    }

    pub fn size(&self) -> u64 {
        self.new_partition_info
            .as_ref()
            .and_then(|info| info.size)
            .unwrap_or_default()
    }

    pub fn hash(&self) -> Option<&[u8]> {
        self.new_partition_info
            .as_ref()
            .and_then(|info| info.hash.as_deref())
            .filter(|hash| !hash.is_empty())
    }

    /// 要下载的数据量（压缩后）
    pub fn download_size(&self) -> u64 {
        self.operations
            .iter()
            .map(|op| op.data_length.unwrap_or_default())
            .sum()
    }

    /// 增量包里的分区要靠旧镜像还原，单靠包本身提不出来
    pub fn is_full(&self) -> bool {
        self.operations.iter().all(|op| {
            matches!(
                op.r#type.unwrap_or(REPLACE),
                REPLACE | REPLACE_BZ | ZERO | DISCARD | REPLACE_XZ | ZSTD
            )
        })
    }
}

pub struct Payload {
    /// 数据区在整个 zip 里的起始偏移
    data_start: u64,
    pub block_size: u64,
    pub partitions: Vec<PartitionUpdate>,
}

impl Payload {
    /// `offset` 是 payload.bin 在 zip 里的起始偏移（必须是 STORED）
    pub async fn open(src: &dyn Source, offset: u64) -> Result<Self, BotError> {
        let header = src.read(offset, 24).await?;
        if &header[..4] != MAGIC {
            return Err(BotError::Custom("payload.bin 格式不对".to_string()));
        }
        let version = u64::from_be_bytes(header[4..12].try_into().unwrap());
        let manifest_size = u64::from_be_bytes(header[12..20].try_into().unwrap());
        let (header_len, signature_size) = match version {
            1 => (20, 0),
            2 => (
                24,
                u32::from_be_bytes(header[20..24].try_into().unwrap()) as u64,
            ),
            _ => {
                return Err(BotError::Custom(format!(
                    "不支持的 payload 版本：{version}"
                )));
            }
        };
        if manifest_size > MAX_MANIFEST {
            return Err(BotError::Custom("payload manifest 过大".to_string()));
        }
        let manifest = src.read(offset + header_len, manifest_size).await?;
        let manifest = DeltaArchiveManifest::decode(manifest)
            .map_err(|err| BotError::Custom(format!("payload manifest 解析失败：{err}")))?;
        Ok(Self {
            data_start: offset + header_len + manifest_size + signature_size,
            block_size: manifest.block_size.unwrap_or(4096) as u64,
            partitions: manifest.partitions,
        })
    }

    pub fn partition(&self, name: &str) -> Option<&PartitionUpdate> {
        self.partitions
            .iter()
            .find(|part| part.name().eq_ignore_ascii_case(name))
    }

    /// 把分区写到 `out`，返回镜像的 sha256。`downloaded` 随下载累加
    pub async fn extract(
        &self,
        src: Arc<dyn Source>,
        part: &PartitionUpdate,
        out: &Path,
        downloaded: Arc<AtomicU64>,
    ) -> Result<[u8; 32], BotError> {
        if !part.is_full() {
            return Err(BotError::Custom(format!(
                "{} 是增量更新，需要旧镜像才能还原，请用全量卡刷包",
                part.name()
            )));
        }
        let file = File::create(out)?;
        file.set_len(part.size())?;
        let file = Arc::new(file);
        let block_size = self.block_size;

        futures::stream::iter(batches(&part.operations))
            .map(|batch| {
                let src = src.clone();
                let file = file.clone();
                let downloaded = downloaded.clone();
                let data_start = self.data_start;
                async move {
                    let first = batch[0].data_offset.unwrap_or_default();
                    let len: u64 = batch
                        .iter()
                        .map(|op| op.data_length.unwrap_or_default())
                        .sum();
                    let data = src.read(data_start + first, len).await?;
                    downloaded.fetch_add(len, Ordering::Relaxed);
                    tokio::task::spawn_blocking(move || {
                        apply(&file, block_size, &batch, first, &data)
                    })
                    .await?
                }
            })
            .buffer_unordered(CONCURRENCY)
            .try_collect::<()>()
            .await?;
        drop(file);

        let out = out.to_path_buf();
        let digest = tokio::task::spawn_blocking(move || sha256_file(&out)).await??;
        if let Some(expected) = part.hash()
            && expected != digest
        {
            return Err(BotError::Custom(format!(
                "{} 校验失败（sha256 不一致），请重试",
                part.name()
            )));
        }
        Ok(digest)
    }
}

/// 按数据偏移把相邻的 operation 合并成一批，每批一个 Range 请求。
/// 没有数据的（ZERO/DISCARD）不用管：文件预先 set_len，空洞就是零
fn batches(ops: &[InstallOperation]) -> Vec<Vec<InstallOperation>> {
    let mut ops: Vec<_> = ops
        .iter()
        .filter(|op| op.data_length.unwrap_or_default() > 0)
        .cloned()
        .collect();
    ops.sort_by_key(|op| op.data_offset.unwrap_or_default());
    let mut batches: Vec<Vec<InstallOperation>> = Vec::new();
    let mut end = 0;
    let mut size = 0;
    for op in ops {
        let offset = op.data_offset.unwrap_or_default();
        let len = op.data_length.unwrap_or_default();
        match batches.last_mut() {
            Some(batch) if offset == end && size + len <= BATCH_BYTES => {
                batch.push(op);
                size += len;
            }
            _ => {
                batches.push(vec![op]);
                size = len;
            }
        }
        end = offset + len;
    }
    batches
}

/// 解压一批数据并写到各自的目标块
fn apply(
    file: &File,
    block_size: u64,
    batch: &[InstallOperation],
    first: u64,
    data: &Bytes,
) -> Result<(), BotError> {
    for op in batch {
        let start = (op.data_offset.unwrap_or_default() - first) as usize;
        let blob = &data[start..start + op.data_length.unwrap_or_default() as usize];
        let expected: u64 = op
            .dst_extents
            .iter()
            .map(|e| e.num_blocks.unwrap_or_default() * block_size)
            .sum();
        let decoded = decode(op.r#type.unwrap_or(REPLACE), blob, expected)?;
        let mut rest = &decoded[..];
        for extent in &op.dst_extents {
            if rest.is_empty() {
                break;
            }
            let len =
                ((extent.num_blocks.unwrap_or_default() * block_size) as usize).min(rest.len());
            file.write_all_at(
                &rest[..len],
                extent.start_block.unwrap_or_default() * block_size,
            )?;
            rest = &rest[len..];
        }
    }
    Ok(())
}

fn decode(kind: i32, blob: &[u8], expected: u64) -> Result<std::borrow::Cow<'_, [u8]>, BotError> {
    let mut out = Vec::with_capacity(expected as usize);
    match kind {
        REPLACE => return Ok(std::borrow::Cow::Borrowed(blob)),
        REPLACE_XZ => {
            lzma_rust2::XzReader::new(blob, true).read_to_end(&mut out)?;
        }
        REPLACE_BZ => {
            bzip2::read::BzDecoder::new(blob).read_to_end(&mut out)?;
        }
        ZSTD => {
            zstd::stream::read::Decoder::new(blob)?.read_to_end(&mut out)?;
        }
        _ => {
            return Err(BotError::Custom(format!("不支持的 operation 类型：{kind}")));
        }
    }
    Ok(std::borrow::Cow::Owned(out))
}

pub fn sha256_file(path: &Path) -> Result<[u8; 32], BotError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// 测试里拼 payload.bin 用
#[cfg(test)]
pub fn build_payload(manifest: &DeltaArchiveManifest, blobs: &[u8]) -> Vec<u8> {
    let manifest = manifest.encode_to_vec();
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&2u64.to_be_bytes());
    out.extend_from_slice(&(manifest.len() as u64).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&manifest);
    out.extend_from_slice(blobs);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::funcs::command::dump::{remote::Memory, zip};
    use std::io::Write;

    fn op(kind: i32, offset: u64, len: u64, start: u64, blocks: u64) -> InstallOperation {
        InstallOperation {
            r#type: Some(kind),
            data_offset: Some(offset),
            data_length: Some(len),
            dst_extents: vec![Extent {
                start_block: Some(start),
                num_blocks: Some(blocks),
            }],
        }
    }

    #[test]
    fn merges_adjacent_operations() {
        let ops = vec![
            op(REPLACE, 10, 5, 0, 1),
            op(REPLACE, 0, 10, 1, 1),
            op(ZERO, 0, 0, 2, 1),
            op(REPLACE, 100, 1, 3, 1),
        ];
        let batches = batches(&ops);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(batches[0][0].data_offset, Some(0));
        assert_eq!(batches[1][0].data_offset, Some(100));
    }

    #[tokio::test]
    async fn extracts_a_partition_from_a_zip() {
        const BS: u64 = 4096;
        // 镜像：块 0 = 0xAA（REPLACE），块 1 = 零（ZERO），块 2 = 0xBB（xz），块 3 = 0xCC（zstd）
        let mut image = vec![0xAA; BS as usize];
        image.extend(vec![0; BS as usize]);
        image.extend(vec![0xBB; BS as usize]);
        image.extend(vec![0xCC; BS as usize]);

        let raw = vec![0xAA; BS as usize];
        let mut xz = Vec::new();
        {
            let mut w =
                lzma_rust2::XzWriter::new(&mut xz, lzma_rust2::XzOptions::default()).unwrap();
            w.write_all(&vec![0xBB; BS as usize]).unwrap();
            w.finish().unwrap();
        }
        let zst = zstd::stream::encode_all(&vec![0xCC; BS as usize][..], 3).unwrap();

        let mut blobs = Vec::new();
        let mut ops = Vec::new();
        for (kind, data, block) in [(REPLACE, &raw, 0), (REPLACE_XZ, &xz, 2), (ZSTD, &zst, 3)] {
            ops.push(op(kind, blobs.len() as u64, data.len() as u64, block, 1));
            blobs.extend_from_slice(data);
        }
        ops.push(op(ZERO, 0, 0, 1, 1));
        let hash: [u8; 32] = Sha256::digest(&image).into();
        let manifest = DeltaArchiveManifest {
            block_size: Some(BS as u32),
            minor_version: Some(0),
            partitions: vec![PartitionUpdate {
                partition_name: Some("init_boot".to_string()),
                new_partition_info: Some(PartitionInfo {
                    size: Some(image.len() as u64),
                    hash: Some(hash.to_vec()),
                }),
                operations: ops,
            }],
        };
        let payload = build_payload(&manifest, &blobs);
        let zip_bytes = Bytes::from(zip::tests::build_zip(&[
            ("META-INF/com/android/metadata", b"x"),
            ("payload.bin", &payload),
        ]));
        let src: Arc<dyn Source> = Arc::new(Memory(zip_bytes.clone()));

        let entries = zip::entries(src.as_ref(), &zip_bytes).await.unwrap();
        let entry = entries.iter().find(|e| e.name == "payload.bin").unwrap();
        let offset = entry.data_offset(src.as_ref()).await.unwrap();
        let payload = Payload::open(src.as_ref(), offset).await.unwrap();
        let part = payload.partition("INIT_BOOT").unwrap().clone();
        assert_eq!(part.size(), 4 * BS);

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("init_boot.img");
        let downloaded = Arc::new(AtomicU64::new(0));
        let digest = payload
            .extract(src, &part, &out, downloaded.clone())
            .await
            .unwrap();
        assert_eq!(digest, hash);
        assert_eq!(std::fs::read(&out).unwrap(), image);
        assert_eq!(downloaded.load(Ordering::Relaxed), part.download_size());
    }
}
