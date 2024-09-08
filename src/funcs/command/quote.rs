use super::*;
use cairo::{Format, ImageSurface};
use image::io::Reader as ImageReader;
use std::io::Cursor;
use teloxide::net::Download as _;

error_fmt!(
    EMPTY,
    DownloadError(teloxide::DownloadError),
    IOError(std::io::Error),
    ImageError(image::ImageError),
    CairoError(cairo::Error),
);

async fn get_user_profile_photos(
    bot: &Bot,
    msg: &Message,
    output_size: (u32, u32),
) -> Result<ImageSurface, AppError> {
    let (width, height) = output_size;
    let photo = bot.get_user_profile_photos(msg.from().unwrap().id).await?;
    let file_id = &photo.photos[0][0].file.id;
    let mut buffer = Vec::new();
    bot.download_file(&file_id, &mut buffer).await?;
    let img = ImageReader::new(Cursor::new(buffer))
        .with_guessed_format()?
        .decode()?;
    let img = img
        .resize(width, height, image::imageops::Lanczos3)
        .to_rgb8();

    // 准备一个向量来存储转换后的图像数据
    let mut data = Vec::with_capacity((width * height * 4) as usize);

    // 转换图像数据到 RGB24 格式，并添加额外的字节
    for pixel in img.pixels() {
        data.push(pixel[2]); // B
        data.push(pixel[1]); // G
        data.push(pixel[0]); // R
        data.push(0); // 填充字节
    }

    // 创建一个ImageSurface从内存中的图像数据
    Ok(ImageSurface::create_for_data(
        data,
        Format::Rgb24,
        width as i32,
        height as i32,
        (width * 4) as i32, // 每行的字节数
    )?)
}


