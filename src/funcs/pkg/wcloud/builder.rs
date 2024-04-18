use super::*;
use image::{codecs::png::PngEncoder, ImageEncoder, ImageError, RgbImage};
use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyList},
};
use std::{collections::HashMap, sync::Arc};

#[derive(Error, Debug)]
pub enum AppError {
    #[error("图像生成失败: {0}")]
    IError(#[from] ImageError),
    #[error("内部错误:{0}")]
    PyError(#[from] PyErr),
}

lazy_static! {
    static ref WCLOUD: Arc<PyObject> = Arc::new(
        generate_wordcloud(
            400,
            400,
            "white",
            "./data/font.ttf",
            "./data/mask.png",
            [
                "#10357B", "#A6BEEC", "#6C81B0", "#092F70", "#A7AAD3", "#758BC4"
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            5,
        )
        .unwrap()
    );
}

fn generate_wordcloud(
    width: i32,
    height: i32,
    background_color: &str,
    font_path: &str,
    mask_path: &str,
    color_list: Vec<String>,
    scale: i32,
) -> PyResult<PyObject> {
    Python::with_gil(|py| {
        let matplotlib_module = PyModule::import_bound(py, "matplotlib.colors")?;
        let py_color_list = PyList::new_bound(py, &color_list);
        let colormap = matplotlib_module
            .getattr("ListedColormap")?
            .call1((py_color_list,))?;

        let wordcloud_module = PyModule::import_bound(py, "wordcloud")?;
        let imageio_module = PyModule::import_bound(py, "imageio")?;
        let mk = imageio_module.getattr("imread")?.call1((mask_path,))?;
        let kwargs = [("width", width), ("height", height)].into_py_dict_bound(py);
        kwargs.set_item("background_color", background_color)?;
        kwargs.set_item("font_path", font_path)?;
        kwargs.set_item("mask", mk)?;
        kwargs.set_item("colormap", colormap)?;
        kwargs.set_item("scale", scale)?;
        let wc = wordcloud_module
            .getattr("WordCloud")?
            .call((), Some(&kwargs))?;
        Ok(wc.to_object(py))
    })
}

pub fn build(mut png_bytes: &mut Vec<u8>, words: HashMap<String, i32>) -> Result<(), AppError> {
    Python::with_gil(|py| -> Result<(), AppError> {
        let image = WCLOUD.call_method1(
            py,
            "generate_from_frequencies",
            (words.into_py_dict_bound(py),),
        )?;
        let pil_image = image.call_method0(py, "to_image")?;

        let pil_pixels = pil_image
            .call_method0(py, "tobytes")?
            .extract::<Vec<u8>>(py)?;

        let size_tuple = pil_image.getattr(py, "size")?.extract::<(u32, u32)>(py)?;
        let (width, height) = size_tuple;

        // 将图像数据转换为 Rust 的 RgbImage
        let rgb_image = RgbImage::from_raw(width, height, pil_pixels).unwrap();

        // 将图像编码为 PNG
        PngEncoder::new(&mut png_bytes).write_image(
            &rgb_image.into_raw(),
            width,
            height,
            image::ColorType::Rgb8.into(),
        )?;
        Ok(())
    })
}
