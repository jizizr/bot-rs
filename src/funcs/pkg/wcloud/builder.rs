use super::*;
use ::wordcloud::{Config, Mask, WordCloud, raster::AbGlyphRasterizer, render};
use rand::prelude::*;
use std::{collections::HashMap, sync::LazyLock};

/// 字体字节：每次生成都要交给一个新的光栅器（`WordCloud` 内部有 `RefCell` 缓存，不是 `Sync`，
/// 不能放进全局静态量）。
static FONT: LazyLock<Vec<u8>> =
    LazyLock::new(|| std::fs::read("./data/font.ttf").expect("Unable to read font file"));

/// 纯白像素为禁止绘制区域，与旧实现一致。
static MASK: LazyLock<Mask> = LazyLock::new(|| {
    let mask = image::open("./data/mask.png")
        .expect("Unable to load mask file")
        .to_luma8();
    Mask::from_pixels(
        mask.width() as usize,
        mask.height() as usize,
        1,
        mask.as_raw(),
    )
    .expect("Invalid mask")
});

const COLORS: [&str; 6] = [
    "#10357B", "#A6BEEC", "#6C81B0", "#092F70", "#A7AAD3", "#758BC4",
];

pub fn build(png_bytes: &mut Vec<u8>, words: HashMap<&str, usize>) -> Result<(), BotError> {
    let config = Config {
        // 布局在遮罩尺寸上进行，输出放大 5 倍
        scale: 5.0,
        max_words: crate::dao::mysql::wordcloud::MAX_WORDS,
        prefer_horizontal: 0.9,
        background_color: Some("white".into()),
        // 不复现 Python 版的历史 bug，不追求逐字节对齐
        legacy_quirks: false,
        ..Config::default()
    };
    let wc = WordCloud::builder()
        .config(config)
        .mask(MASK.clone())
        .rasterizer(Box::new(AbGlyphRasterizer::from_bytes(FONT.clone(), true)?))
        .build()?;

    // 频率降序、同频按词升序，保证输入顺序稳定
    let mut freqs: Vec<(String, f64)> = words
        .into_iter()
        .map(|(w, f)| (w.to_owned(), f as f64))
        .collect();
    freqs.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut layout = wc.generate_from_frequencies(&freqs)?;
    let mut rng = rand::rng();
    for word in &mut layout.words {
        word.color = COLORS[rng.random_range(0..COLORS.len())].to_owned();
    }
    render::write_png(&render::to_image(&wc, &layout)?, png_bytes)?;
    Ok(())
}
