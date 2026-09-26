use std::io::{BufReader, Read};
use std::path::Path;

use tauri::State;

use crate::db;

use super::{AppState, MapErrStr};

/// 只有多帧容器才有"动图"可言：jpg/bmp 开都不用开，按扩展名先筛一轮
const ANIMATED_CANDIDATES: &[&str] = &["png", "gif", "webp", "avif"];

/// 扫描库里的多帧动图（GIF / APNG / 动态 WebP / AVIF 序列），返回命中的图片 id。
/// 判据全部走文件头结构解析，不依赖 ffmpeg；结果由前端高亮并勾选，删除仍走回收站那条通路。
#[tauri::command]
pub async fn find_animated_images(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let jobs: Vec<(String, String)> = {
        let conn = state.db.lock().map_err_str()?;
        db::get_all_images(&conn)
            .map_err_str()?
            .into_iter()
            .filter(|img| {
                Path::new(&img.path)
                    .extension()
                    .map(|e| {
                        ANIMATED_CANDIDATES.iter().any(|c| e.eq_ignore_ascii_case(c))
                    })
                    .unwrap_or(false)
            })
            .map(|img| (img.id, img.path))
            .collect()
    };
    tauri::async_runtime::spawn_blocking(move || {
        jobs.into_iter()
            .filter(|(_, path)| is_animated(Path::new(path)))
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    })
    .await
    .map_err_str()
}

/// 打不开或解析失败一律当"不是动图"：宁可漏报（用户看不见高亮），不能误报让人删错
fn is_animated(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    animated_from(&mut BufReader::new(file)).unwrap_or(false)
}

/// 从流当前位置按魔数分派。调用方保证流在文件开头。
/// 魔数必须分段读：读到哪就从哪分派，流的位置才正好接上该格式的后续结构（一次读满 12 字节会把 GIF 的 LSD、PNG 的首块长度一起吞掉）。
fn animated_from<R: Read>(r: &mut R) -> std::io::Result<bool> {
    let mut head = [0u8; 12];
    if fill(r, &mut head[..6])? < 6 {
        return Ok(false);
    }
    if head.starts_with(b"GIF8") {
        return gif_animated(r); // 恰好消费 6 字节头
    }
    if fill(r, &mut head[6..8])? + 6 < 8 {
        return Ok(false);
    }
    if head[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return png_animated(r); // 恰好消费 8 字节头
    }
    if fill(r, &mut head[8..12])? + 8 < 12 {
        return Ok(false);
    }
    if &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return webp_animated(r); // 恰好消费 RIFF+size+WEBP
    }
    if &head[4..8] == b"ftyp" {
        // ISO-BMFF：major brand 或兼容品牌里带 "avis"（image sequence）即动画。
        // 至此已消费 box size(4) + "ftyp"(4) + major(4)
        let major = &head[8..12];
        let box_size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        // 剩下 minor(4) + 兼容品牌；上限防损坏文件读飞
        let rest = (box_size.saturating_sub(12)).min(512) as usize;
        let mut brands = vec![0u8; rest];
        let got = fill(r, &mut brands)?;
        if major == b"avis" {
            return Ok(true);
        }
        return Ok(brands[..got].chunks_exact(4).any(|b| b == b"avis"));
    }
    Ok(false)
}

/// 尽量填满 buf，返回实际读到的字节数（短文件允许不足）
fn fill<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        let n = r.read(&mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(got)
}

/// 读 n 字节并丢弃；读不到就当文件截断，交给上层按"非动图"处理
fn skip<R: Read>(r: &mut R, mut n: usize) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    while n > 0 {
        let take = n.min(buf.len());
        let got = fill(r, &mut buf[..take])?;
        if got == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        n -= got;
    }
    Ok(())
}

/// GIF 数据子块链：每块 1 字节数长 + 数据，0 长块收尾。扩展与图像数据都用它跳
fn skip_sub_blocks<R: Read>(r: &mut R) -> std::io::Result<()> {
    loop {
        let mut len = [0u8; 1];
        fill(r, &mut len)?;
        if len[0] == 0 {
            return Ok(());
        }
        skip(r, len[0] as usize)?;
    }
}

/// GIF：帧数 ≥2 才是动图。头 6 字节魔数已被消费。
fn gif_animated<R: Read>(r: &mut R) -> std::io::Result<bool> {
    let mut lsd = [0u8; 7];
    fill(r, &mut lsd)?;
    let packed = lsd[4];
    if packed & 0x80 != 0 {
        let gct = 3 * (1usize << ((packed & 0x07) as usize + 1));
        skip(r, gct)?;
    }
    let mut frames = 0usize;
    loop {
        let mut marker = [0u8; 1];
        fill(r, &mut marker)?;
        match marker[0] {
            // 图像描述符：9 字节描述 + 可选局部色表 + LZW 数据
            0x2C => {
                let mut desc = [0u8; 9];
                fill(r, &mut desc)?;
                if desc[8] & 0x80 != 0 {
                    let lct = 3 * (1usize << ((desc[8] & 0x07) as usize + 1));
                    skip(r, lct)?;
                }
                skip(r, 1)?; // LZW 最小码长
                skip_sub_blocks(r)?;
                frames += 1;
                if frames >= 2 {
                    return Ok(true);
                }
            }
            // 扩展：标签后接子块链（注释/图形控制/NETSCAPE 循环等）
            0x21 => {
                let mut label = [0u8; 1];
                fill(r, &mut label)?;
                skip_sub_blocks(r)?;
            }
            // 文件尾：到此只有一帧
            0x3B => return Ok(false),
            // 魔数之后不是合法块标记：损坏文件，不当动图
            _ => return Ok(false),
        }
    }
}

/// PNG：APNG 的 acTL 块必须排在 IDAT 之前；走到 IDAT 还没有 acTL 就是静态图
fn png_animated<R: Read>(r: &mut R) -> std::io::Result<bool> {
    loop {
        let mut len_buf = [0u8; 4];
        fill(r, &mut len_buf)?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut kind = [0u8; 4];
        fill(r, &mut kind)?;
        match &kind {
            b"acTL" => {
                let mut frames = [0u8; 4];
                fill(r, &mut frames)?;
                return Ok(u32::from_be_bytes(frames) > 1);
            }
            b"IDAT" | b"IEND" => return Ok(false),
            _ => {
                skip(r, len + 4)?; // 数据 + CRC
            }
        }
    }
}

/// WebP：VP8X 的 animation 标志位或 ANIM 块；走到无损/有损图像块即静态
fn webp_animated<R: Read>(r: &mut R) -> std::io::Result<bool> {
    loop {
        let mut fourcc = [0u8; 4];
        fill(r, &mut fourcc)?;
        let mut size_buf = [0u8; 4];
        fill(r, &mut size_buf)?;
        let size = u32::from_le_bytes(size_buf) as usize;
        match &fourcc {
            b"ANIM" => return Ok(true),
            b"VP8X" => {
                let mut payload = vec![0u8; size.min(1024)];
                let got = fill(r, &mut payload)?;
                let animated = got > 0 && payload[0] & 0x02 != 0;
                if animated {
                    return Ok(true);
                }
                skip(r, size.saturating_sub(got))?;
            }
            b"VP8 " | b"VP8L" => return Ok(false),
            _ => {
                skip(r, size)?;
                if size % 2 == 1 {
                    skip(r, 1)?; // RIFF 块按偶数字节对齐
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sniff(bytes: &[u8]) -> bool {
        animated_from(&mut Cursor::new(bytes)).unwrap_or(false)
    }

    /// 造一张 n 帧的最小合法 GIF（无色表、每帧 1×1）
    fn gif_frames(frames: usize) -> Vec<u8> {
        let mut b = b"GIF89a".to_vec();
        b.extend_from_slice(&[1, 0, 1, 0, 0x00, 0, 0]); // LSD，无全局色表
        for _ in 0..frames {
            b.push(0x2C);
            b.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0, 0x00]); // 图像描述符
            b.push(0x02); // LZW 最小码长
            b.extend_from_slice(&[2, 0x4C, 0x01]); // 一个数据子块
            b.push(0x00); // 子块链收尾
        }
        b.push(0x3B);
        b
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut b = (data.len() as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(data);
        b.extend_from_slice(&[0, 0, 0, 0]); // CRC 不参与判据
        b
    }

    fn png_with(chunks: Vec<Vec<u8>>) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        for c in chunks {
            b.extend_from_slice(&c);
        }
        b
    }

    fn webp_with(chunks: Vec<(&[u8; 4], Vec<u8>)>) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        for (fourcc, data) in chunks {
            body.extend_from_slice(fourcc);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(&data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(&body);
        b
    }

    fn ftyp(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
        let body_len = 4 + 4 + compat.len() * 4; // major + minor + 兼容品牌
        let mut b = ((body_len + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(b"ftyp");
        b.extend_from_slice(major);
        b.extend_from_slice(&[0, 0, 0, 0]); // minor
        for brand in compat {
            b.extend_from_slice(*brand);
        }
        b
    }

    #[test]
    fn test_gif_frame_count_is_the_animation_judge() {
        assert!(!sniff(&gif_frames(1)), "单帧 GIF 不该算动图");
        assert!(sniff(&gif_frames(2)), "两帧 GIF 该算动图");
        assert!(sniff(&gif_frames(9)), "多帧 GIF 该算动图");
    }

    #[test]
    fn test_gif_with_extensions_and_global_color_table() {
        let mut b = b"GIF89a".to_vec();
        // 带全局色表（2 色）+ 注释扩展 + 图形控制扩展，随后两帧
        b.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]);
        b.extend_from_slice(&[0, 0, 0, 255, 255, 255]); // GCT 6 字节
        b.push(0x21);
        b.push(0xFE); // 注释标签
        b.extend_from_slice(&[3, b'a', b'b', b'c', 0]);
        b.push(0x21);
        b.push(0xF9); // 图形控制标签
        b.extend_from_slice(&[4, 0, 0, 0, 0, 0]);
        for _ in 0..2 {
            b.push(0x2C);
            b.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0, 0x00]);
            b.push(0x02);
            b.extend_from_slice(&[2, 0x4C, 0x01]);
            b.push(0x00);
        }
        b.push(0x3B);
        assert!(sniff(&b), "扩展块跳过有误，多帧没数出来");
    }

    #[test]
    fn test_png_actl_before_idat_is_animation() {
        let ihdr = png_chunk(b"IHDR", &[0; 13]);
        let actl = png_chunk(b"acTL", &[0, 0, 0, 3, 0, 0, 0, 1]);
        let idat = png_chunk(b"IDAT", &[1, 2, 3]);
        let animated = png_with(vec![ihdr.clone(), actl, idat.clone()]);
        assert!(sniff(&animated), "3 帧 APNG 该算动图");

        let actl1 = png_chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 1]);
        let single_frame = png_with(vec![ihdr.clone(), actl1, idat.clone()]);
        assert!(!sniff(&single_frame), "num_frames=1 不算动图");

        let plain = png_with(vec![ihdr, idat]);
        assert!(!sniff(&plain), "普通 PNG 不算动图");
    }

    #[test]
    fn test_webp_animation_flag_and_anim_chunk() {
        let static_vp8 = webp_with(vec![(b"VP8 ", vec![0; 10])]);
        assert!(!sniff(&static_vp8), "有损静态 WebP 不算动图");

        // VP8X 首字节 flags：bit1(0x02) = animation
        let mut flags = vec![0u8; 10];
        flags[0] = 0x02;
        let anim_flag = webp_with(vec![(b"VP8X", flags), (b"ANMF", vec![0; 16])]);
        assert!(sniff(&anim_flag), "VP8X 动画标志该算动图");

        let no_flag = webp_with(vec![(b"VP8X", vec![0; 10]), (b"VP8 ", vec![0; 8])]);
        assert!(!sniff(&no_flag), "VP8X 无动画标志不算动图");
    }

    #[test]
    fn test_avif_sequence_brand_is_animation() {
        assert!(sniff(&ftyp(b"avis", &[])), "major brand avis 该算动图");
        assert!(sniff(&ftyp(b"avif", &[b"mif1", b"avis"])), "兼容品牌 avis 该算动图");
        assert!(!sniff(&ftyp(b"avif", &[b"MIAF"])), "静态 AVIF 不算动图");
    }

    #[test]
    fn test_other_formats_and_truncated_files_are_not_animation() {
        assert!(!sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0]), "JPEG 不算动图");
        assert!(!sniff(b"BM"), "BMP 不算动图（短文件也不该 panic）");
        assert!(!sniff(&gif_frames(1)[..10]), "截断文件按非动图处理");
        let mut corrupt = gif_frames(1).to_vec();
        corrupt.truncate(8);
        assert!(!sniff(&corrupt), "只有魔数的残文件按非动图处理");
    }

    #[test]
    fn test_is_animated_reads_real_files_and_treats_missing_as_static() {
        let dir = std::env::temp_dir().join(format!("viewman-anim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.gif");
        std::fs::write(&p, gif_frames(3)).unwrap();
        assert!(is_animated(&p), "盘上的多帧 GIF 该被判为动图");

        let missing = dir.join("nope.gif");
        assert!(!is_animated(&missing), "文件不存在按非动图处理，不该报错");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
