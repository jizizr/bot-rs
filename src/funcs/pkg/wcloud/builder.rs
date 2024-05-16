extern crate wcloud as wc;
use super::*;
use image::{ImageBuffer, ImageError, ImageFormat, Luma};
use std::{
    collections::HashMap,
    fs::File,
    io::{Cursor, Read as _},
};
use wc::{Tokenizer, WordCloud, WordCloudSize};
#[derive(Error, Debug)]
pub enum AppError {
    #[error("图像生成失败: {0}")]
    IError(#[from] ImageError),
}

lazy_static! {
    static ref WCLOUD: WordCloud = {
        let tokenizer = Tokenizer::default();
        WordCloud::default()
            .with_tokenizer(tokenizer)
            .with_word_rotate_chance(0.1)
            .with_font_from_path("./data/font.ttf".into())
    };
    static ref MASK: ImageBuffer<Luma<u8>, Vec<u8>> = {
        let mut file = File::open("./data/mask.png").expect("Unable to open mask file");
        let mut mask_buf = Vec::new();
        file.read_to_end(&mut mask_buf)
            .expect("Unable to read mask file");
        image::load_from_memory_with_format(&mask_buf, ImageFormat::Png)
            .expect("Unable to load mask from memory")
            .to_luma8()
    };
}

pub fn build(png_bytes: &mut Vec<u8>, words: HashMap<&str, usize>) -> Result<(), AppError> {
    let start = std::time::Instant::now();
    let rgb_image = WCLOUD.generate_from_map(words, WordCloudSize::FromMask(MASK.clone()), 5.0);
    rgb_image.write_to(&mut Cursor::new(png_bytes), ImageFormat::Png)?;
    println!("生成词云耗时: {:?}", start.elapsed());
    Ok(())
}
