//! PDQ 感知哈希：Meta/Facebook 开源的行业标准图片指纹（256 位），对重编码、缩放、
//! 轻度压缩噪声的稳健性都好于 64 位 pHash/dHash 组合，距离粒度也更细（0–256）。
//!
//! vendor 自 darwinium-com/pdqhash 0.1.1（Apache-2.0）：
//! https://github.com/darwinium-com/pdqhash
//! 改动只有一处：剥掉 `image` crate 依赖——本项目的解码统一走 ffmpeg 管线，
//! 这里只收现成的灰度像素。算法主体（Jarosz 盒滤波 → 64×64 → DCT 左上 16×16 →
//! 中值阈值化 256 位）与参考实现逐行一致，并有用参考 crate 逐位对拍的测试兜底。
//!
//! 对外入口：
//! - [`pdq_from_gray64`]：ffmpeg 出的 64×64 灰度（生产路径）
//! - [`pdq_from_luma`]：任意尺寸 luma 缓冲（对拍测试 + 通用）

// vendor 保真：DCT/滤波循环与除法公式按参考实现逐行保留（有逐位对拍测试背书），
// clippy 的改写建议会破坏「与参考实现一眼可比对」这个性质
#![allow(clippy::needless_range_loop, clippy::manual_div_ceil)]

// - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -
// 最小可哈希尺寸（参考实现同款：任一边不足 5 像素没有结构可言）
const MIN_HASHABLE_DIM: usize = 5;

const PDQ_NUM_JAROSZ_XY_PASSES: usize = 2;

const BUFFER_W_H: usize = 64;
const DCT_OUTPUT_W_H: usize = 16;
const DCT_OUTPUT_MATRIX_SIZE: usize = DCT_OUTPUT_W_H * DCT_OUTPUT_W_H;
/// 256 位 = 32 字节
pub const HASH_LENGTH: usize = DCT_OUTPUT_MATRIX_SIZE / 8;

// - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -
// Jarosz 盒滤波（先行后列各跑 nreps 趟，分离式滑动窗口均值）
fn jarosz_filter_float(
    buffer1: &mut [f32], // 行主序矩阵
    num_rows: usize,
    num_cols: usize,
    window_size_along_rows: usize,
    window_size_along_cols: usize,
    nreps: usize,
) {
    let mut temp_buf = Vec::new();
    temp_buf.resize(buffer1.len(), 0.0);
    for _ in 0..nreps {
        box_along_rows_float(
            buffer1,
            temp_buf.as_mut_slice(),
            num_rows,
            num_cols,
            window_size_along_rows,
        );
        box_along_cols_float(
            temp_buf.as_slice(),
            buffer1,
            num_rows,
            num_cols,
            window_size_along_cols,
        );
    }
}

// 这个函数有两个调用点（固定步长/可变步长），编译器各特化一份
#[inline(always)]
fn box_one_d_float(
    invec: &[f32],
    in_start_offset: usize,
    outvec: &mut [f32],
    vector_length: usize,
    stride: usize,
    full_window_size: usize,
) {
    let half_window_size = (full_window_size + 2) / 2; // 7->4, 8->5

    let phase_1_nreps = half_window_size - 1;
    let phase_2_nreps = full_window_size - half_window_size + 1;

    let oi_off = phase_1_nreps * stride;
    let li_off = phase_2_nreps * stride;

    let mut sum = 0.0;
    let mut current_window_size = 0.0;

    let phase_1_end = oi_off + in_start_offset;

    // PHASE 1: 只累计不写
    for ri in (in_start_offset..phase_1_end).step_by(stride) {
        let value = invec[ri];
        sum += value;
        current_window_size += 1.0;
    }

    let phase_2_end = full_window_size * stride + in_start_offset;
    // PHASE 2: 小窗口起步写
    for ri in (phase_1_end..phase_2_end).step_by(stride) {
        let oi = ri - oi_off;
        sum += invec[ri];
        current_window_size += 1.0;
        outvec[oi] = sum / current_window_size;
    }

    let phase_3_end = vector_length * stride + in_start_offset;
    // PHASE 3: 满窗滑动写
    for ri in (phase_2_end..phase_3_end).step_by(stride) {
        let oi = ri - oi_off;
        let li = oi - li_off;
        sum += invec[ri];
        sum -= invec[li];
        outvec[oi] = sum / (current_window_size);
    }

    let phase_4_start = (vector_length - half_window_size + 1) * stride + in_start_offset;
    // PHASE 4: 收尾小窗口写
    for oi in (phase_4_start..phase_3_end).step_by(stride) {
        let li = oi - li_off;
        sum -= invec[li];
        current_window_size -= 1.0;
        outvec[oi] = sum / current_window_size;
    }
}

fn box_along_rows_float(
    input: &[f32],
    output: &mut [f32],
    n_rows: usize,
    n_cols: usize,
    window_size: usize,
) {
    for i in 0..n_rows {
        box_one_d_float(input, i * n_cols, output, n_cols, 1, window_size);
    }
}

fn box_along_cols_float(
    input: &[f32],
    output: &mut [f32],
    n_rows: usize,
    n_cols: usize,
    window_size: usize,
) {
    for j in 0..n_cols {
        box_one_d_float(input, j, output, n_rows, n_cols, window_size);
    }
}

fn compute_jarosz_filter_window_size(old_dimension: usize, new_dimension: usize) -> usize {
    (old_dimension + 2 * new_dimension - 1) / (2 * new_dimension)
}

fn decimate_float<const OUT_NUM_ROWS: usize, const OUT_NUM_COLS: usize>(
    input: &[f32],
    in_num_rows: usize,
    in_num_cols: usize,
) -> [[f32; OUT_NUM_COLS]; OUT_NUM_ROWS] {
    let mut output = [[0.0; OUT_NUM_COLS]; OUT_NUM_ROWS];
    // 取中心不取角落
    for outi in 0..OUT_NUM_ROWS {
        let ini = ((outi * 2 + 1) * in_num_rows) / (OUT_NUM_ROWS * 2);
        for outj in 0..OUT_NUM_COLS {
            let inj = ((outj * 2 + 1) * in_num_cols) / (OUT_NUM_COLS * 2);
            output[outi][outj] = input[ini * in_num_cols + inj];
        }
    }
    output
}

// - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -
// 纯启发式的画质度量（见 PDQ 文档）：数"显著梯度"而不是把小抖动也算进来，
// 常数都是参考实现手工调过的。哈希判定不用它，调用方可拿来过滤平图。
fn pdq_image_domain_quality_metric<const OUT_NUM_ROWS: usize, const OUT_NUM_COLS: usize>(
    buffer64x64: &[[f32; OUT_NUM_COLS]; OUT_NUM_ROWS],
) -> f32 {
    let mut gradient_sum = 0.0;

    for i in 0..(OUT_NUM_ROWS - 1) {
        for j in 0..OUT_NUM_COLS {
            let u = buffer64x64[i][j];
            let v = buffer64x64[i + 1][j];
            let d = (u - v) / 255.;
            gradient_sum += d.abs();
        }
    }
    for i in 0..OUT_NUM_ROWS {
        for j in 0..(OUT_NUM_COLS - 1) {
            let u = buffer64x64[i][j];
            let v = buffer64x64[i][j + 1];
            let d = (u - v) / 255.;
            gradient_sum += d.abs();
        }
    }

    let quality = gradient_sum / 90.;
    if quality > 1.0 {
        1.0
    } else {
        quality
    }
}

// 从 64×64 矩阵做 DCT，只算左上 16×16 角，比算全量快
fn dct64_to_16<const OUT_NUM_ROWS: usize, const OUT_NUM_COLS: usize>(
    input: &[[f32; OUT_NUM_COLS]; OUT_NUM_ROWS],
) -> [f32; DCT_OUTPUT_MATRIX_SIZE] {
    let mut intermediate_matrix = [[0.0; OUT_NUM_COLS]; DCT_OUTPUT_W_H];
    for i in 0..DCT_OUTPUT_W_H {
        for j in 0..OUT_NUM_COLS {
            let mut sumk = 0.0;
            for k in 0..BUFFER_W_H {
                sumk += f32::from_bits(DCT_MATRIX[i][k]) * input[k][j];
            }

            intermediate_matrix[i][j] = sumk;
        }
    }

    let mut output = [0.0; DCT_OUTPUT_MATRIX_SIZE];
    for i in 0..DCT_OUTPUT_W_H {
        for j in 0..DCT_OUTPUT_W_H {
            let mut sumk = 0.0;
            for k in 0..BUFFER_W_H {
                sumk += intermediate_matrix[i][k] * f32::from_bits(DCT_MATRIX[j][k]);
            }
            output[i * DCT_OUTPUT_W_H + j] = sumk;
        }
    }
    output
}

// 快速中位数（Torben）
fn torben_median(m: &[f32]) -> Option<f32> {
    let mut min = m.iter().cloned().reduce(f32::min)?;
    let mut max = m.iter().cloned().reduce(f32::max)?;

    let half = (m.len() + 1) / 2;
    loop {
        let guess = (min + max) / 2.0;
        let mut less = 0;
        let mut greater = 0;
        let mut equal = 0;
        let mut maxltguess = min;
        let mut mingtguess = max;
        for val in m {
            if *val < guess {
                less += 1;
                if *val > maxltguess {
                    maxltguess = *val;
                }
            } else if *val > guess {
                greater += 1;
                if *val < mingtguess {
                    mingtguess = *val;
                }
            } else {
                equal += 1;
            }
        }
        if less <= half && greater <= half {
            return Some(if less >= half {
                maxltguess
            } else if less + equal >= half {
                guess
            } else {
                mingtguess
            });
        } else if less > greater {
            max = maxltguess;
        } else {
            min = mingtguess;
        }
    }
}

fn pdq_buffer16x16_to_bits(input: &[f32; DCT_OUTPUT_MATRIX_SIZE]) -> [u8; HASH_LENGTH] {
    let dct_median = torben_median(input).expect("16×16 恒非空");
    let mut hash = [0; HASH_LENGTH];

    for i in 0..HASH_LENGTH {
        let mut byte = 0;
        for j in 0..8 {
            let val = input[i * 8 + j];
            if val > dct_median {
                byte |= 1 << j;
            }
        }
        hash[HASH_LENGTH - i - 1] = byte;
    }
    hash
}

// - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -
// DCT 预乘矩阵（64×64 DCT 只需左上 16 行），逐位原样来自参考实现
pub const DCT_MATRIX: [[u32; BUFFER_W_H]; DCT_OUTPUT_W_H] = [
    [
        0x3e34f6fe, 0x3e348763, 0x3e33a872, 0x3e325ab4, 0x3e309ef6, 0x3e2e764c, 0x3e2be209,
        0x3e28e3c6, 0x3e257d59, 0x3e21b0dd, 0x3e1d80a8, 0x3e18ef51, 0x3e13ffa8, 0x3e0eb4b9,
        0x3e0911c7, 0x3e031a4c, 0x3df9a3ec, 0x3dec794b, 0x3ddebcd5, 0x3dd076fe, 0x3dc1b099,
        0x3db272be, 0x3da2c6d4, 0x3d92b688, 0x3d824bc0, 0x3d632139, 0x3d411ee4, 0x3d1ea56f,
        0x3cf79447, 0x3cb144ff, 0x3c5510c6, 0x3b8e284e, 0xbb8e286f, 0xbc5510d6, 0xbcb14507,
        0xbcf79450, 0xbd1ea573, 0xbd411ee8, 0xbd63213c, 0xbd824bc2, 0xbd92b689, 0xbda2c6d6,
        0xbdb272bd, 0xbdc1b09a, 0xbdd076fe, 0xbddebcd7, 0xbdec794d, 0xbdf9a3f1, 0xbe031a4d,
        0xbe0911c7, 0xbe0eb4b9, 0xbe13ffa8, 0xbe18ef52, 0xbe1d80a9, 0xbe21b0dd, 0xbe257d59,
        0xbe28e3c6, 0xbe2be20a, 0xbe2e764c, 0xbe309ef7, 0xbe325ab4, 0xbe33a871, 0xbe348763,
        0xbe34f6fe,
    ],
    [
        0x3e34cd21, 0x3e330f61, 0x3e2f982b, 0x3e2a700c, 0x3e23a3b9, 0x3e1b43f5, 0x3e116566,
        0x3e062061, 0x3df3215b, 0x3dd7aa8b, 0x3dba2004, 0x3d9aca9f, 0x3d73ef2e, 0x3d2fefb8,
        0x3cd47d0b, 0x3c0e1d77, 0xbc0e1d88, 0xbcd47d12, 0xbd2fefbb, 0xbd73ef32, 0xbd9aca9e,
        0xbdba2005, 0xbdd7aa8c, 0xbdf3215e, 0xbe062062, 0xbe116567, 0xbe1b43f5, 0xbe23a3b9,
        0xbe2a700c, 0xbe2f982b, 0xbe330f61, 0xbe34cd21, 0xbe34cd21, 0xbe330f61, 0xbe2f982b,
        0xbe2a700c, 0xbe23a3b8, 0xbe1b43f4, 0xbe116566, 0xbe062061, 0xbdf3215b, 0xbdd7aa88,
        0xbdba2006, 0xbd9aca9a, 0xbd73ef2f, 0xbd2fefae, 0xbcd47d03, 0xbc0e1d39, 0x3c0e1d98,
        0x3cd47d04, 0x3d2fefc5, 0x3d73ef31, 0x3d9acaa5, 0x3dba2006, 0x3dd7aa8a, 0x3df3215f,
        0x3e062061, 0x3e116568, 0x3e1b43f6, 0x3e23a3ba, 0x3e2a700c, 0x3e2f982b, 0x3e330f62,
        0x3e34cd21,
    ],
    [
        0x3e348763, 0x3e309ef6, 0x3e28e3c6, 0x3e1d80a8, 0x3e0eb4b9, 0x3df9a3ec, 0x3dd07700,
        0x3da2c6d4, 0x3d63213e, 0x3cf79447, 0x3b8e287b, 0xbcb14507, 0xbd411ee2, 0xbd92b689,
        0xbdc1b09a, 0xbdec794d, 0xbe0911c7, 0xbe18ef50, 0xbe257d59, 0xbe2e764c, 0xbe33a871,
        0xbe34f6fe, 0xbe325ab4, 0xbe2be209, 0xbe21b0dd, 0xbe13ffa9, 0xbe031a4c, 0xbddebcd5,
        0xbdb272be, 0xbd824bc3, 0xbd1ea576, 0xbc5510f9, 0x3c5510fd, 0x3d1ea578, 0x3d824bc4,
        0x3db272be, 0x3ddebcd5, 0x3e031a4c, 0x3e13ffa7, 0x3e21b0dc, 0x3e2be20b, 0x3e325ab4,
        0x3e34f6fe, 0x3e33a871, 0x3e2e764c, 0x3e257d59, 0x3e18ef52, 0x3e0911c8, 0x3dec7949,
        0x3dc1b095, 0x3d92b687, 0x3d411ee1, 0x3cb14505, 0xbb8e2857, 0xbcf7943f, 0xbd63214a,
        0xbda2c6cf, 0xbdd07703, 0xbdf9a3e7, 0xbe0eb4b9, 0xbe1d80ab, 0xbe28e3c5, 0xbe309ef8,
        0xbe348763,
    ],
    [
        0x3e3425ce, 0x3e2d3986, 0x3e1fa511, 0x3e0bee0a, 0x3de5acc6, 0x3daaa9f0, 0x3d523043,
        0x3c8df1a8, 0xbc8df1b0, 0xbd523047, 0xbdaaa9f4, 0xbde5acc6, 0xbe0bee0a, 0xbe1fa513,
        0xbe2d3986, 0xbe3425ce, 0xbe3425ce, 0xbe2d3986, 0xbe1fa511, 0xbe0bee09, 0xbde5acc7,
        0xbdaaa9f1, 0xbd52303f, 0xbc8df195, 0x3c8df1c4, 0x3d523055, 0x3daaa9f1, 0x3de5acc7,
        0x3e0bee0b, 0x3e1fa513, 0x3e2d3987, 0x3e3425cf, 0x3e3425ce, 0x3e2d3986, 0x3e1fa510,
        0x3e0bee08, 0x3de5acc0, 0x3daaa9ea, 0x3d523046, 0x3c8df1a3, 0xbc8df1b5, 0xbd523050,
        0xbdaaa9ee, 0xbde5accd, 0xbe0bee0a, 0xbe1fa515, 0xbe2d3986, 0xbe3425cf, 0xbe3425ce,
        0xbe2d3986, 0xbe1fa50f, 0xbe0bee0a, 0xbde5acbb, 0xbdaaa9ed, 0xbd52304d, 0xbc8df184,
        0x3c8df1a7, 0x3d52305e, 0x3daaa9f5, 0x3de5acd3, 0x3e0bee0c, 0x3e1fa511, 0x3e2d3988,
        0x3e3425ce,
    ],
    [
        0x3e33a872, 0x3e28e3c6, 0x3e13ffa8, 0x3dec794b, 0x3da2c6d4, 0x3d1ea56f, 0xbb8e289d,
        0xbd411ee8, 0xbdb272c2, 0xbdf9a3f1, 0xbe18ef52, 0xbe2be20a, 0xbe348763, 0xbe325ab4,
        0xbe257d59, 0xbe0eb4b9, 0xbddebcd5, 0xbd92b683, 0xbcf7944a, 0x3c5510fd, 0x3d632151,
        0x3dc1b09c, 0x3e031a4f, 0x3e1d80a9, 0x3e2e764d, 0x3e34f6fe, 0x3e309ef6, 0x3e21b0dd,
        0x3e0911c4, 0x3dd076f5, 0x3d824bbc, 0x3cb144d8, 0xbcb14518, 0xbd824bcb, 0xbdd07703,
        0xbe0911c6, 0xbe21b0e0, 0xbe309ef8, 0xbe34f6fe, 0xbe2e764a, 0xbe1d80a5, 0xbe031a4a,
        0xbdc1b097, 0xbd63211d, 0xbc55107e, 0x3cf7945c, 0x3d92b69d, 0x3ddebce1, 0x3e0eb4bb,
        0x3e257d59, 0x3e325ab5, 0x3e348763, 0x3e2be209, 0x3e18ef4b, 0x3df9a3e1, 0x3db272b4,
        0x3d411eda, 0x3b8e2714, 0xbd1ea58f, 0xbda2c6de, 0xbdec7961, 0xbe13ffae, 0xbe28e3c8,
        0xbe33a872,
    ],
    [
        0x3e330f61, 0x3e23a3b9, 0x3e062061, 0x3dba2004, 0x3d2fefbd, 0xbc0e1d88, 0xbd73ef2c,
        0xbdd7aa8c, 0xbe116566, 0xbe2a700c, 0xbe34cd21, 0xbe2f982b, 0xbe1b43f6, 0xbdf3215b,
        0xbd9aca9a, 0xbcd47d03, 0x3cd47d04, 0x3d9aca9a, 0x3df3215f, 0x3e1b43f6, 0x3e2f982b,
        0x3e34cd21, 0x3e2a700b, 0x3e116565, 0x3dd7aa8b, 0x3d73ef36, 0x3c0e1d55, 0xbd2fefbe,
        0xbdba2004, 0xbe06205f, 0xbe23a3b8, 0xbe330f60, 0xbe330f60, 0xbe23a3b7, 0xbe06205f,
        0xbdba2004, 0xbd2fefbc, 0x3c0e1d5f, 0x3d73ef22, 0x3dd7aa84, 0x3e11656a, 0x3e2a700d,
        0x3e34cd21, 0x3e2f982b, 0x3e1b43f5, 0x3df3215e, 0x3d9acaa4, 0x3cd47d2d, 0xbcd47d34,
        0xbd9acaa5, 0xbdf3215f, 0xbe1b43f6, 0xbe2f982b, 0xbe34cd21, 0xbe2a700d, 0xbe116562,
        0xbdd7aa95, 0xbd73ef20, 0xbc0e1e06, 0x3d2fefc0, 0x3dba2017, 0x3e062060, 0x3e23a3bd,
        0x3e330f60,
    ],
    [
        0x3e325ab4, 0x3e1d80a8, 0x3dec794b, 0x3d824bc0, 0x3b8e284e, 0xbd632142, 0xbddebcd7,
        0xbe18ef52, 0xbe309ef7, 0xbe33a871, 0xbe21b0db, 0xbdf9a3e9, 0xbd92b683, 0xbc55109f,
        0x3d411ef2, 0x3dd07706, 0x3e13ffaa, 0x3e2e764d, 0x3e348763, 0x3e257d57, 0x3e031a49,
        0x3da2c6cb, 0x3cb144d8, 0xbd1ea571, 0xbdc1b0ad, 0xbe0eb4b9, 0xbe2be20d, 0xbe34f6fe,
        0xbe28e3c1, 0xbe0911c6, 0xbdb272a6, 0xbcf79439, 0x3cf794b5, 0x3db272c2, 0x3e0911d0,
        0x3e28e3c7, 0x3e34f6ff, 0x3e2be209, 0x3e0eb4af, 0x3dc1b092, 0x3d1ea533, 0xbcb14528,
        0xbda2c6f1, 0xbe031a50, 0xbe257d60, 0xbe348763, 0xbe2e7648, 0xbe13ffa4, 0xbdd076e2,
        0xbd411eca, 0x3c5511f5, 0x3d92b695, 0x3df9a409, 0x3e21b0e0, 0x3e33a874, 0x3e309ef5,
        0x3e18ef47, 0x3ddebcc8, 0x3d6320f1, 0xbb8e2991, 0xbd824be8, 0xbdec795b, 0xbe1d80b3,
        0xbe325ab5,
    ],
    [
        0x3e318a85, 0x3e168317, 0x3dc9234d, 0x3d0d42ab, 0xbd0d42af, 0xbdc92351, 0xbe168318,
        0xbe318a86, 0xbe318a85, 0xbe168316, 0xbdc92348, 0xbd0d42b2, 0x3d0d42b3, 0x3dc92352,
        0x3e168317, 0x3e318a86, 0x3e318a85, 0x3e168314, 0x3dc9234b, 0x3d0d42a3, 0xbd0d42ac,
        0xbdc9234f, 0xbe168319, 0xbe318a86, 0xbe318a84, 0xbe168312, 0xbdc9234f, 0xbd0d42aa,
        0x3d0d42bb, 0x3dc92357, 0x3e16831b, 0x3e318a87, 0x3e318a86, 0x3e168316, 0x3dc92348,
        0x3d0d429a, 0xbd0d42ca, 0xbdc9235c, 0xbe168317, 0xbe318a86, 0xbe318a85, 0xbe168314,
        0xbdc92355, 0xbd0d428c, 0x3d0d42ad, 0x3dc92363, 0x3e168319, 0x3e318a89, 0x3e318a84,
        0x3e168319, 0x3dc9233c, 0x3d0d42a9, 0xbd0d42e9, 0xbdc92357, 0xbe168315, 0xbe318a87,
        0xbe318a86, 0xbe168310, 0xbdc92348, 0xbd0d426d, 0x3d0d42cb, 0x3dc9234a, 0x3e16831d,
        0x3e318a86,
    ],
    [
        0x3e309ef6, 0x3e0eb4b9, 0x3da2c6d4, 0x3b8e287b, 0xbd92b689, 0xbe0911c7, 0xbe2e764c,
        0xbe325ab4, 0xbe13ffa9, 0xbdb272be, 0xbc5510f9, 0x3d824bc4, 0x3e031a4c, 0x3e2be20b,
        0x3e33a871, 0x3e18ef52, 0x3dc1b095, 0x3cb14505, 0xbd63214a, 0xbdf9a3e7, 0xbe28e3c5,
        0xbe348763, 0xbe1d80ab, 0xbdd07701, 0xbcf79439, 0x3d411ef9, 0x3dec794a, 0x3e257d59,
        0x3e34f6fe, 0x3e21b0de, 0x3ddebcd3, 0x3d1ea560, 0xbd1ea562, 0xbddebcd5, 0xbe21b0de,
        0xbe34f6fe, 0xbe257d59, 0xbdec7959, 0xbd411eca, 0x3cf79440, 0x3dd076f0, 0x3e1d80ab,
        0x3e348763, 0x3e28e3c9, 0x3df9a3e5, 0x3d632147, 0xbcb144b2, 0xbdc1b0a0, 0xbe18ef4f,
        0xbe33a873, 0xbe2be209, 0xbe031a4f, 0xbd824bad, 0x3c551107, 0x3db272b6, 0x3e13ffae,
        0x3e325ab4, 0x3e2e764d, 0x3e0911c0, 0x3d92b685, 0xbb8e27b4, 0xbda2c6e5, 0xbe0eb4b9,
        0xbe309ef5,
    ],
    [
        0x3e2f982b, 0x3e062060, 0x3d73ef2e, 0xbcd47d12, 0xbdd7aa8c, 0xbe23a3b9, 0xbe34cd21,
        0xbe1b43f4, 0xbdba1ffd, 0xbc0e1d39, 0x3d9acaa5, 0x3e116568, 0x3e330f62, 0x3e2a700b,
        0x3df3215a, 0x3d2fefb5, 0xbd2fefbe, 0xbdf32165, 0xbe2a700c, 0xbe330f60, 0xbe116560,
        0xbd9aca96, 0x3c0e1e14, 0x3dba200b, 0x3e1b43f9, 0x3e34cd21, 0x3e23a3b6, 0x3dd7aa88,
        0x3cd47cd3, 0xbd73ef5c, 0xbe062065, 0xbe2f982e, 0xbe2f982a, 0xbe06205a, 0xbd73ef20,
        0x3cd47cf9, 0x3dd7aaa2, 0x3e23a3bd, 0x3e34cd21, 0x3e1b43ec, 0x3dba1fee, 0x3c0e1d14,
        0xbd9aca9f, 0xbe116570, 0xbe330f62, 0xbe2a700a, 0xbdf3213c, 0xbd2fef80, 0x3d2fefdd,
        0x3df32161, 0x3e2a7012, 0x3e330f5f, 0x3e116562, 0x3d9aca73, 0xbc0e1e94, 0xbdba2019,
        0xbe1b43f8, 0xbe34cd22, 0xbe23a3b2, 0xbdd7aa7b, 0xbcd47c3a, 0x3d73ef7a, 0x3e06206a,
        0x3e2f982d,
    ],
    [
        0x3e2e764c, 0x3df9a3ec, 0x3d1ea56a, 0xbd632142, 0xbe0911c9, 0xbe325ab4, 0xbe28e3c5,
        0xbddebccb, 0xbcb144f7, 0x3d92b68e, 0x3e13ffaa, 0x3e348763, 0x3e21b0da, 0x3dc1b08b,
        0x3b8e2757, 0xbdb272cf, 0xbe1d80ab, 0xbe34f6fe, 0xbe18ef4d, 0xbda2c6ba, 0x3c551178,
        0x3dd07708, 0x3e257d5e, 0x3e33a871, 0x3e0eb4af, 0x3d824bad, 0xbcf794d5, 0xbdec7961,
        0xbe2be20c, 0xbe309ef3, 0xbe031a45, 0xbd411eca, 0x3d411f42, 0x3e031a5a, 0x3e309efa,
        0x3e2be206, 0x3dec7943, 0x3cf79385, 0xbd824be8, 0xbe0eb4c2, 0xbe33a873, 0xbe257d4d,
        0xbdd076d5, 0xbc550f87, 0x3da2c6f2, 0x3e18ef57, 0x3e34f6ff, 0x3e1d809c, 0x3db27299,
        0xbb8e2a88, 0xbdc1b0d3, 0xbe21b0ea, 0xbe348761, 0xbe13ff9c, 0xbd92b66a, 0x3cb1461b,
        0x3ddebd06, 0x3e28e3cf, 0x3e325ab0, 0x3e0911ac, 0x3d63210c, 0xbd1ea5e9, 0xbdf9a434,
        0xbe2e7652,
    ],
    [
        0x3e2d3986, 0x3de5acc6, 0x3c8df1a8, 0xbdaaa9f4, 0xbe1fa511, 0xbe3425ce, 0xbe0bee0b,
        0xbd52303f, 0x3d523040, 0x3e0bee0b, 0x3e3425ce, 0x3e1fa510, 0x3daaa9f4, 0xbc8df1b5,
        0xbde5accd, 0xbe2d3986, 0xbe2d3986, 0xbde5accc, 0xbc8df184, 0x3daaa9f5, 0x3e1fa511,
        0x3e3425cf, 0x3e0bee07, 0x3d52303f, 0xbd523042, 0xbe0bee07, 0xbe3425cf, 0xbe1fa510,
        0xbdaaa9f3, 0x3c8df18b, 0x3de5acbc, 0x3e2d3983, 0x3e2d3983, 0x3de5acbb, 0x3c8df182,
        0xbdaaa9f6, 0xbe1fa511, 0xbe3425cf, 0xbe0bee0e, 0xbd523068, 0x3d52306d, 0x3e0bee0e,
        0x3e3425cf, 0x3e1fa510, 0x3daaa9f3, 0xbc8df18d, 0xbde5acbc, 0xbe2d3983, 0xbe2d3983,
        0xbde5acb9, 0xbc8df180, 0x3daaa9f6, 0x3e1fa511, 0x3e3425cf, 0x3e0bee0e, 0x3d523010,
        0xbd523019, 0xbe0bee0f, 0xbe3425cd, 0xbe1fa510, 0xbdaaa9ca, 0x3c8df18f, 0x3de5acdf,
        0x3e2d3983,
    ],
    [
        0x3e2be209, 0x3dd076fe, 0xbb8e286f, 0xbddebcd7, 0xbe2e764c, 0xbe28e3c5, 0xbdc1b092,
        0x3c5510fd, 0x3dec794e, 0x3e309ef6, 0x3e257d57, 0x3db272b7, 0xbcb14518, 0xbdf9a3f8,
        0xbe325ab4, 0xbe21b0db, 0xbda2c6cf, 0x3cf7945c, 0x3e031a4d, 0x3e33a872, 0x3e1d80a3,
        0x3d92b678, 0xbd1ea58f, 0xbe0911cc, 0xbe348763, 0xbe18ef4e, 0xbd824bbb, 0x3d411eec,
        0x3e0eb4b9, 0x3e34f6fe, 0x3e13ffa9, 0x3d632147, 0xbd632184, 0xbe13ffb2, 0xbe34f6fd,
        0xbe0eb4af, 0xbd411eac, 0x3d824bda, 0x3e18ef57, 0x3e348762, 0x3e0911c0, 0x3d1ea54f,
        0xbd92b696, 0xbe1d80ab, 0xbe33a871, 0xbe031a49, 0xbcf79435, 0x3da2c6d8, 0x3e21b0e7,
        0x3e325ab0, 0x3df9a3f1, 0x3cb14469, 0xbdb272b6, 0xbe257d60, 0xbe309ef8, 0xbdec7936,
        0xbc550e91, 0x3dc1b0ae, 0x3e28e3d2, 0x3e2e7649, 0x3ddebca3, 0x3b8e2775, 0xbdd0772f,
        0xbe2be20b,
    ],
    [
        0x3e2a700c, 0x3dba2004, 0xbcd47d12, 0xbe062062, 0xbe34cd21, 0xbe116565, 0xbd2fefae,
        0x3d9acaa5, 0x3e23a3ba, 0x3e2f982a, 0x3dd7aa83, 0xbc0e1dd5, 0xbdf32165, 0xbe330f62,
        0xbe1b43f2, 0xbd73ef11, 0x3d73ef4d, 0x3e1b43f9, 0x3e330f5f, 0x3df3214e, 0x3c0e1cda,
        0xbdd7aa9c, 0xbe2f982e, 0xbe23a3b8, 0xbd9aca74, 0x3d2fefc0, 0x3e116575, 0x3e34cd21,
        0x3e062050, 0x3cd47cf0, 0xbdba2032, 0xbe2a700d, 0xbe2a7002, 0xbdba1ffd, 0x3cd47de9,
        0x3e062065, 0x3e34cd22, 0x3e116562, 0x3d2fef45, 0xbd9acaad, 0xbe23a3c7, 0xbe2f9829,
        0xbdd7aa57, 0x3c0e1e1d, 0x3df3218c, 0x3e330f62, 0x3e1b43e3, 0x3d73ef02, 0xbd73efb3,
        0xbe1b43fc, 0xbe330f5b, 0xbdf32147, 0xbc0e1b2a, 0x3dd7aaa3, 0x3e2f9835, 0x3e23a3b2,
        0x3d9aca57, 0xbd2feffc, 0xbe11657e, 0xbe34cd20, 0xbe062045, 0xbcd47c73, 0x3dba204d,
        0x3e2a7013,
    ],
    [
        0x3e28e3c6, 0x3da2c6d4, 0xbd411ee2, 0xbe18ef50, 0xbe325ab4, 0xbddebcd5, 0x3c5510fd,
        0x3e031a4c, 0x3e34f6fe, 0x3e0911c8, 0x3cb14505, 0xbdd07703, 0xbe309ef8, 0xbe1d80ab,
        0xbd632147, 0x3d92b687, 0x3e257d59, 0x3e2be209, 0x3db272b4, 0xbd1ea562, 0xbe13ffa7,
        0xbe33a871, 0xbdec7959, 0x3b8e27ab, 0x3df9a3e8, 0x3e348763, 0x3e0eb4b9, 0x3cf79438,
        0xbdc1b0a0, 0xbe2e764e, 0xbe21b0d8, 0xbd824bad, 0x3d824baf, 0x3e21b0d9, 0x3e2e764d,
        0x3dc1b09d, 0xbcf79443, 0xbe0eb4b9, 0xbe348763, 0xbdf9a3e5, 0xbb8e277d, 0x3dec795b,
        0x3e33a873, 0x3e13ffad, 0x3d1ea531, 0xbdb272b6, 0xbe2be210, 0xbe257d59, 0xbd92b6ae,
        0x3d63214d, 0x3e1d80a0, 0x3e309ef5, 0x3dd07712, 0xbcb1456a, 0xbe0911c2, 0xbe34f6fd,
        0xbe031a4e, 0xbc550f7e, 0x3ddebcd6, 0x3e325ab0, 0x3e18ef4e, 0x3d411f1f, 0xbda2c6e6,
        0xbe28e3c2,
    ],
    [
        0x3e273d74, 0x3d8a8bd4, 0xbd8a8bd6, 0xbe273d75, 0xbe273d74, 0xbd8a8bcc, 0x3d8a8bd8,
        0x3e273d75, 0x3e273d73, 0x3d8a8bd0, 0xbd8a8bdf, 0xbe273d74, 0xbe273d73, 0xbd8a8bc9,
        0x3d8a8bd1, 0x3e273d75, 0x3e273d72, 0x3d8a8bc1, 0xbd8a8bd8, 0xbe273d77, 0xbe273d75,
        0xbd8a8bcf, 0x3d8a8bdf, 0x3e273d78, 0x3e273d6f, 0x3d8a8bb3, 0xbd8a8bd1, 0xbe273d75,
        0xbe273d72, 0xbd8a8bc1, 0x3d8a8bee, 0x3e273d7b, 0x3e273d75, 0x3d8a8bcf, 0xbd8a8be0,
        0xbe273d78, 0xbe273d6f, 0xbd8a8bb3, 0x3d8a8bd2, 0x3e273d75, 0x3e273d72, 0x3d8a8bc0,
        0xbd8a8bc4, 0xbe273d7b, 0xbe273d75, 0xbd8a8ba4, 0x3d8a8be0, 0x3e273d81, 0x3e273d6f,
        0x3d8a8bdc, 0xbd8a8bfd, 0xbe273d75, 0xbe273d69, 0xbd8a8bc0, 0x3d8a8bc5, 0x3e273d7b,
        0x3e273d75, 0x3d8a8ba4, 0xbd8a8be1, 0xbe273d81, 0xbe273d6f, 0xbd8a8bdb, 0x3d8a8bfd,
        0x3e273d75,
    ],
];
// __DCT_MATRIX_REST__

// - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -
// 对外入口

/// 任意尺寸 luma 缓冲走完整 PDQ 链（Jarosz → 64×64 → DCT → 256 位）。
/// 与参考实现 `generate_pdq_full_size` 对同一份 luma 输入逐位一致。
pub fn pdq_from_luma(luma: &[f32], rows: usize, cols: usize) -> Option<([u8; HASH_LENGTH], f32)> {
    if rows < MIN_HASHABLE_DIM || cols < MIN_HASHABLE_DIM {
        return None;
    }
    let mut image = luma.to_vec();
    let window_size_along_rows = compute_jarosz_filter_window_size(cols, BUFFER_W_H);
    let window_size_along_cols = compute_jarosz_filter_window_size(rows, BUFFER_W_H);

    jarosz_filter_float(
        image.as_mut_slice(),
        rows,
        cols,
        window_size_along_rows,
        window_size_along_cols,
        PDQ_NUM_JAROSZ_XY_PASSES,
    );

    let buffer64x64 = decimate_float::<BUFFER_W_H, BUFFER_W_H>(image.as_slice(), rows, cols);
    let buffer16x16 = dct64_to_16(&buffer64x64);
    Some((
        pdq_buffer16x16_to_bits(&buffer16x16),
        pdq_image_domain_quality_metric(&buffer64x64),
    ))
}

/// ffmpeg 出的 64×64 灰度（0–255）直接进 DCT 段。
/// 到手已经是目标尺寸，Jarosz（窗 1）与抽取（64→64）在参考实现里都是恒等映射，
/// 走全链保持同一条代码路径。
pub fn pdq_from_gray64(gray: &[u8; BUFFER_W_H * BUFFER_W_H]) -> ([u8; HASH_LENGTH], f32) {
    let luma: Vec<f32> = gray.iter().map(|&b| b as f32).collect();
    pdq_from_luma(&luma, BUFFER_W_H, BUFFER_W_H).expect("64×64 一定可哈希")
}

/// 256 位哈希的汉明距离
pub fn pdq_distance(a: &[u8; HASH_LENGTH], b: &[u8; HASH_LENGTH]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性伪随机（LCG），测试里当"噪点图"用
    fn noise(seed: u64, rows: usize, cols: usize) -> Vec<u8> {
        let mut s = seed;
        (0..rows * cols)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (s >> 33) as u8
            })
            .collect()
    }

    /// 渐变图：值随列线性增长
    fn gradient(rows: usize, cols: usize) -> Vec<u8> {
        (0..rows * cols)
            .map(|i| ((i % cols) * 255 / cols.max(1)) as u8)
            .collect()
    }

    #[test]
    fn flat_image_quality_is_zero_and_hash_stable() {
        // 全平图：画质（梯度）为 0；哈希的位来自 DCT 系数对中值的浮点噪声比较，
        // 不是稳定语义，所以只断言确定性而不是"全 0"
        let gray = [128u8; 64 * 64];
        let (hash, quality) = pdq_from_gray64(&gray);
        let (hash2, _) = pdq_from_gray64(&gray);
        assert_eq!(hash, hash2, "同输入必须同哈希");
        assert_eq!(quality, 0.0, "平图无梯度，画质应为 0");
    }

    #[test]
    fn deterministic_and_distance_sane() {
        let a: Vec<u8> = noise(1, 64, 64);
        let b: Vec<u8> = noise(2, 64, 64);
        let (ha, _) = pdq_from_gray64(a.as_slice().try_into().unwrap());
        let (ha2, _) = pdq_from_gray64(&a.try_into().unwrap());
        assert_eq!(ha, ha2, "同输入必须同哈希");

        let (hb, _) = pdq_from_gray64(&b.try_into().unwrap());
        let d = pdq_distance(&ha, &hb);
        // 256 位里两张无关噪点图应差到随机水平（期望 ≈128），绝不可能是近重复
        assert!(d > 90, "无关噪点图距离 {d} 太近");
        assert_eq!(pdq_distance(&ha, &ha), 0);
    }

    #[test]
    fn brightness_shift_moves_few_bits() {
        // 整体亮度漂移主要落在 DC 分量上，256 位里只该翻动零星几位
        let base: Vec<u8> = noise(7, 64, 64);
        let lifted: Vec<u8> = base.iter().map(|&v| v.saturating_add(20)).collect();
        let (h1, _) = pdq_from_gray64(&base.try_into().unwrap());
        let (h2, _) = pdq_from_gray64(&lifted.try_into().unwrap());
        let d = pdq_distance(&h1, &h2);
        assert!(d <= 24, "亮度漂移距离 {d} 过大");
    }

    #[test]
    fn too_small_images_are_rejected() {
        let tiny = vec![64u8; 4 * 4];
        assert!(pdq_from_luma(
            &tiny.into_iter().map(f32::from).collect::<Vec<_>>(),
            4,
            4
        )
        .is_none());
    }

    /// 与参考 crate（pdqhash 0.1.1）逐位对拍：同一份 luma 缓冲，两边哈希必须完全一致。
    /// 这是 vendor 正确性的最终背书——算法改坏一位都会在这里炸。
    #[test]
    fn matches_reference_pdqhash_bit_for_bit() {
        let cases: Vec<(usize, usize, Vec<u8>)> = vec![
            (64, 64, noise(11, 64, 64)),
            (100, 87, noise(12, 100, 87)),
            (63, 65, gradient(63, 65)),
            (300, 200, noise(13, 300, 200)),
            (17, 5, noise(14, 17, 5)),
        ];
        for (rows, cols, pixels) in cases {
            let luma: Vec<f32> = pixels.iter().map(|&b| b as f32).collect();
            let (mine, my_quality) = pdq_from_luma(&luma, rows, cols).expect("尺寸达标");

            let img = image::ImageBuffer::from_fn(cols as u32, rows as u32, |x, y| {
                image::Luma([pixels[(y as usize) * cols + x as usize]])
            });
            let dyn_img = image::DynamicImage::ImageLuma8(img);
            let (reference, ref_quality) = pdqhash::generate_pdq_full_size(&dyn_img);
            assert_eq!(mine, reference, "{rows}×{cols} 哈希与参考实现不一致");
            assert!(
                (mine.len() == reference.len()) && (my_quality - ref_quality).abs() < 1e-6,
                "{rows}×{cols} quality 漂移：{my_quality} vs {ref_quality}"
            );
        }
    }
}
