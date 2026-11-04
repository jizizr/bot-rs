//! 把一条消息画成 Telegram 聊天气泡样式的贴纸，思路来自 LyoSU/quote-bot：
//! 头像 + 带尾巴的气泡，气泡里是昵称（按 Telegram 配色）、群标签/头衔、正文和时间。
//!
//! 字体优先读 `./data/fonts/` 下的 ttf/otf/ttc（仓库自带裁剪过的 Noto Sans SC 和
//! Noto Color Emoji），缺字时回落到系统字体。

use std::{cell::RefCell, io::Cursor, ops::Range, path::Path};

use skia_safe::{
    AlphaType, BlurStyle, Canvas, ClipOp, Color, Color4f, ColorType, Data, FilterMode, FontMgr,
    FontStyle, Image, ImageInfo, MaskFilter, MipmapMode, Paint, PathBuilder, PathOp, RRect, Rect,
    SamplingOptions, TileMode, Vector,
    font_style::{Slant, Weight, Width},
    gradient::{Colors, Gradient, Interpolation, shaders},
    images, surfaces,
    textlayout::{
        FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextDecoration,
        TextStyle, TypefaceFontProvider,
    },
};

use crate::BotError;

const FONT_DIR: &str = "./data/fonts";
// 仓库自带的 Noto 字体排在最前，各平台出图一致；系统字体只补裁剪掉的生僻字
const FAMILIES: &[&str] = &[
    "Noto Sans SC",
    "Noto Color Emoji",
    "Noto Sans CJK SC",
    "PingFang SC",
    "Microsoft YaHei",
    "Apple Color Emoji",
    "sans-serif",
];
const MONO_FAMILIES: &[&str] = &[
    "Menlo",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "monospace",
    "Noto Sans SC",
    "Noto Color Emoji",
];

/// 贴纸最长边必须是 512
const STICKER_SIDE: f32 = 512.0;

// 以下尺寸都是逻辑像素，最后整体缩放到贴纸尺寸
const MARGIN: f32 = 4.0;
const AVATAR: f32 = 42.0;
const AVATAR_GAP: f32 = 10.0;
const TAIL: f32 = 8.0;
const RADIUS: f32 = 17.0;
const PAD_X: f32 = 12.0;
const PAD_TOP: f32 = 7.0;
const PAD_BOTTOM: f32 = 7.0;
const NAME_SIZE: f32 = 15.5;
const TEXT_SIZE: f32 = 16.0;
const META_SIZE: f32 = 13.0;
const TAG_SIZE: f32 = 13.5;
const TAG_PAD_X: f32 = 7.0;
const TAG_HEIGHT: f32 = 20.0;
const BADGE_GAP: f32 = 14.0;
const NAME_TEXT_GAP: f32 = 2.0;
const TIME_GAP: f32 = 12.0;
const MIN_CONTENT: f32 = 60.0;
const MAX_CONTENT: f32 = 330.0;
// 图片铺满气泡宽度；太高的图收窄，再高就居中裁掉
const PHOTO_MIN_WIDTH: f32 = 160.0;
const PHOTO_MAX_WIDTH: f32 = 300.0;
const PHOTO_MAX_HEIGHT: f32 = 360.0;
const PHOTO_GAP: f32 = 5.0;
const CAPTION_GAP: f32 = 6.0;
// 贴纸最长边，和 Telegram 里差不多大
const STICKER_BOX: f32 = 180.0;
const NAME_PILL_PAD_X: f32 = 10.0;
const NAME_PILL_PAD_Y: f32 = 4.0;
const NAME_PILL_GAP: f32 = 4.0;
const TIME_PILL_HEIGHT: f32 = 18.0;
const TIME_PILL_PAD_X: f32 = 6.0;
const TIME_PILL_INSET: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// 正文里一段文字的样式，对应 Telegram 的消息实体
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub monospace: bool,
    pub link: bool,
    pub spoiler: bool,
}

impl SpanStyle {
    fn merge(self, other: Self) -> Self {
        Self {
            bold: self.bold || other.bold,
            italic: self.italic || other.italic,
            underline: self.underline || other.underline,
            strikethrough: self.strikethrough || other.strikethrough,
            monospace: self.monospace || other.monospace,
            link: self.link || other.link,
            spoiler: self.spoiler || other.spoiler,
        }
    }
}

/// `range` 是正文里的字节区间，两端必须落在字符边界上
#[derive(Clone, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub style: SpanStyle,
}

/// 昵称右侧的标记
#[derive(Clone, Debug)]
pub enum Badge {
    /// 群成员标签，画成彩色胶囊
    Tag(String),
    /// 管理员头衔，灰色文字
    Title(String),
}

#[derive(Clone, Debug)]
pub struct Quote {
    pub name: String,
    /// Telegram 七种基础昵称色之一（0..7）
    pub color_index: usize,
    pub badge: Option<Badge>,
    /// 头像原始图片（jpg/png/webp），没有就画首字母
    pub avatar: Option<Vec<u8>>,
    /// 图片或贴纸的说明文字，没有就是空串
    pub text: String,
    pub spans: Vec<Span>,
    pub time: String,
    pub media: Option<Media>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    /// 放在气泡里，铺满气泡宽度
    Photo,
    /// 不带气泡，直接画在头像旁边
    Sticker,
}

/// 消息里的图片或静态贴纸（jpg/png/webp 原始数据）；解不开就当没有
#[derive(Clone, Debug)]
pub struct Media {
    pub kind: MediaKind,
    pub bytes: Vec<u8>,
}

struct Palette {
    bubble: Color,
    text: Color,
    meta: Color,
    link: Color,
    tag: Color,
    tag_alpha: f32,
    shadow: Option<Color>,
    names: [Color; 7],
}

impl Palette {
    fn of(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self {
                bubble: Color::WHITE,
                text: Color::BLACK,
                meta: Color::from_rgb(0x8c, 0x99, 0xa5),
                link: Color::from_rgb(0x16, 0x8a, 0xcd),
                tag: Color::from_rgb(0x3a, 0xa5, 0x4a),
                tag_alpha: 0.16,
                shadow: Some(Color::from_argb(0x30, 0, 0, 0)),
                names: [
                    Color::from_rgb(0xd0, 0x45, 0x3e),
                    Color::from_rgb(0xd8, 0x74, 0x1b),
                    Color::from_rgb(0x8e, 0x52, 0xd6),
                    Color::from_rgb(0x3a, 0xa5, 0x2a),
                    Color::from_rgb(0x2a, 0x9a, 0xb6),
                    Color::from_rgb(0x2f, 0x83, 0xcf),
                    Color::from_rgb(0xc5, 0x2f, 0x6c),
                ],
            },
            Theme::Dark => Self {
                bubble: Color::from_rgb(0x21, 0x21, 0x21),
                text: Color::from_rgb(0xe8, 0xe8, 0xe8),
                meta: Color::from_rgb(0x84, 0x88, 0x8c),
                link: Color::from_rgb(0x6a, 0xb3, 0xf3),
                tag: Color::from_rgb(0x4c, 0xb5, 0x55),
                tag_alpha: 0.14,
                shadow: None,
                names: [
                    Color::from_rgb(0xe6, 0x5c, 0x55),
                    Color::from_rgb(0xec, 0x8b, 0x3a),
                    Color::from_rgb(0xa5, 0x7b, 0xf0),
                    Color::from_rgb(0x54, 0xbd, 0x45),
                    Color::from_rgb(0x3d, 0xb5, 0xd0),
                    Color::from_rgb(0x4b, 0x9c, 0xe8),
                    Color::from_rgb(0xc8, 0x3a, 0x70),
                ],
            },
        }
    }
}

/// 没有头像时的渐变底色（上、下），和昵称色一一对应
const AVATAR_GRADIENTS: [(u32, u32); 7] = [
    (0xFF885E, 0xFF516A),
    (0xFFCD6A, 0xFFA85C),
    (0xE0A2F3, 0xD669ED),
    (0xA0DE7E, 0x54CB68),
    (0x53EDD6, 0x28C9B7),
    (0x72D5FD, 0x2A9EF1),
    (0xFFA8A8, 0xFF719A),
];

thread_local! {
    // 字体集合不能跨线程，每个渲染线程各建一份
    static FONTS: RefCell<FontCollection> = RefCell::new(font_collection());
}

fn font_collection() -> FontCollection {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::new(), None);
    match bundled_fonts() {
        Some(provider) => collection.set_asset_font_manager(Some(provider.into())),
        None => log::error!("{FONT_DIR} 下没有可用字体，语录只能依赖系统字体"),
    }
    collection
}

fn bundled_fonts() -> Option<TypefaceFontProvider> {
    let manager = FontMgr::new();
    let mut provider = TypefaceFontProvider::new();
    let mut bundled = 0;
    for path in std::fs::read_dir(FONT_DIR)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
    {
        if !is_font_file(&path) {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => match manager.new_from_data(Data::new_copy(&bytes), None) {
                Some(typeface) => {
                    let family = typeface.family_name();
                    provider.register_typeface(typeface, Some(base_family(&family)));
                    bundled += 1;
                }
                None => log::error!("语录字体无法解析：{}", path.display()),
            },
            Err(e) => log::error!("读取语录字体失败 {}：{e}", path.display()),
        }
    }
    (bundled > 0).then_some(provider)
}

/// 旧式命名会把字重写进族名（如 "Noto Sans SC Medium"），
/// 去掉后才能和同族其他字重归到一起按字重挑选
fn base_family(family: &str) -> &str {
    const WEIGHTS: [&str; 9] = [
        " Thin",
        " ExtraLight",
        " Light",
        " DemiLight",
        " Regular",
        " Medium",
        " SemiBold",
        " Bold",
        " Black",
    ];
    WEIGHTS
        .iter()
        .find_map(|weight| family.strip_suffix(weight))
        .unwrap_or(family)
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
}

fn text_style(size: f32, color: Color, weight: Weight) -> TextStyle {
    let mut style = TextStyle::new();
    style
        .set_font_size(size)
        .set_color(color)
        .set_font_families(FAMILIES)
        .set_font_style(FontStyle::new(weight, Width::NORMAL, Slant::Upright));
    style
}

fn paragraph(
    fonts: &FontCollection,
    text: &str,
    style: &TextStyle,
    single_line: bool,
    align: TextAlign,
) -> Paragraph {
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_style(style).set_text_align(align);
    if single_line {
        paragraph_style.set_max_lines(1).set_ellipsis("…");
    }
    let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);
    builder.push_style(style);
    builder.add_text(text);
    builder.build()
}

fn span_style(base: &TextStyle, span: SpanStyle, palette: &Palette) -> TextStyle {
    let mut style = base.clone();
    let weight = if span.bold {
        Weight::BOLD
    } else {
        Weight::NORMAL
    };
    let slant = if span.italic {
        Slant::Italic
    } else {
        Slant::Upright
    };
    style.set_font_style(FontStyle::new(weight, Width::NORMAL, slant));
    if span.monospace {
        style
            .set_font_families(MONO_FAMILIES)
            .set_font_size(TEXT_SIZE * 0.92);
    }
    if span.link {
        style.set_color(palette.link);
    }
    let mut decoration = TextDecoration::NO_DECORATION;
    if span.underline {
        decoration |= TextDecoration::UNDERLINE;
    }
    if span.strikethrough {
        decoration |= TextDecoration::LINE_THROUGH;
    }
    if decoration != TextDecoration::NO_DECORATION {
        style.set_decoration_type(decoration);
        style.set_decoration_color(style.color());
    }
    if span.spoiler {
        // 剧透内容不显示，只留一块遮罩
        let mut mask = Paint::default();
        mask.set_color(palette.meta).set_alpha_f(0.45);
        style
            .set_color(Color::TRANSPARENT)
            .set_background_paint(&mask);
    }
    style
}

fn body_paragraph(fonts: &FontCollection, quote: &Quote, palette: &Palette) -> Paragraph {
    let base = text_style(TEXT_SIZE, palette.text, Weight::NORMAL);
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_style(&base);
    let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);

    let text = quote.text.as_str();
    let in_text = |index: usize| index <= text.len() && text.is_char_boundary(index);
    let spans: Vec<&Span> = quote
        .spans
        .iter()
        .filter(|span| {
            span.range.start < span.range.end
                && in_text(span.range.start)
                && in_text(span.range.end)
        })
        .collect();
    let mut cuts = vec![0, text.len()];
    cuts.extend(
        spans
            .iter()
            .flat_map(|span| [span.range.start, span.range.end]),
    );
    cuts.sort_unstable();
    cuts.dedup();
    for window in cuts.windows(2) {
        let (start, end) = (window[0], window[1]);
        let style = spans
            .iter()
            .filter(|span| span.range.start <= start && end <= span.range.end)
            .fold(SpanStyle::default(), |acc, span| acc.merge(span.style));
        builder.push_style(&span_style(&base, style, palette));
        builder.add_text(&text[start..end]);
        builder.pop();
    }
    builder.build()
}

/// 首字母：名字第一个字，若是多个拉丁单词再取最后一个单词的首字母
fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|word| word.chars().next().is_some_and(char::is_alphanumeric))
        .collect();
    let Some(first) = words.first().and_then(|word| word.chars().next()) else {
        return name.chars().next().map(String::from).unwrap_or_default();
    };
    let mut initials: String = first.to_uppercase().collect();
    let last = words[1..].last().and_then(|word| word.chars().next());
    if let Some(last) =
        last.filter(|last| first.is_ascii_alphabetic() && last.is_ascii_alphabetic())
    {
        initials.extend(last.to_uppercase());
    }
    initials
}

fn draw_avatar(canvas: &skia_safe::Canvas, fonts: &FontCollection, rect: Rect, quote: &Quote) {
    canvas.save();
    canvas.clip_rrect(RRect::new_oval(rect), ClipOp::Intersect, true);
    let photo = quote
        .avatar
        .as_deref()
        .and_then(|bytes| Image::from_encoded(Data::new_copy(bytes)));
    match photo {
        Some(image) => {
            // 非正方形的图居中裁成正方形
            let side = image.width().min(image.height()) as f32;
            let src = Rect::from_xywh(
                (image.width() as f32 - side) / 2.0,
                (image.height() as f32 - side) / 2.0,
                side,
                side,
            );
            let image = image.with_default_mipmaps().unwrap_or(image);
            canvas.draw_image_rect_with_sampling_options(
                &image,
                Some((&src, skia_safe::canvas::SrcRectConstraint::Fast)),
                rect,
                SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
                &Paint::default(),
            );
        }
        None => {
            let (top, bottom) = AVATAR_GRADIENTS[quote.color_index % AVATAR_GRADIENTS.len()];
            let colors = [
                Color4f::from(Color::new(0xFF00_0000 | top)),
                Color4f::from(Color::new(0xFF00_0000 | bottom)),
            ];
            let gradient = Gradient::new(
                Colors::new_evenly_spaced(&colors, TileMode::Clamp, None),
                Interpolation::default(),
            );
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_shader(shaders::linear_gradient(
                ((rect.left, rect.top), (rect.left, rect.bottom)),
                &gradient,
                None,
            ));
            canvas.draw_rect(rect, &paint);

            let style = text_style(rect.width() * 0.4, Color::WHITE, Weight::SEMI_BOLD);
            let mut letters = paragraph(
                fonts,
                &initials(&quote.name),
                &style,
                true,
                TextAlign::Center,
            );
            letters.layout(rect.width());
            letters.paint(
                canvas,
                (rect.left, rect.center_y() - letters.height() / 2.0),
            );
        }
    }
    canvas.restore();
}

/// 左下角是直角并带一条往左弯的尾巴
fn bubble_path(rect: Rect) -> skia_safe::Path {
    let round = Vector::new(RADIUS, RADIUS);
    let rrect = RRect::new_rect_radii(rect, &[round, round, round, Vector::new(0.0, 0.0)]);
    let mut body = PathBuilder::new();
    body.add_rrect(rrect, None, None);
    let body = body.detach();

    let (left, bottom) = (rect.left, rect.bottom);
    let mut tail = PathBuilder::new();
    tail.move_to((left + 1.0, bottom - 18.0))
        .line_to((left, bottom - 18.0))
        .cubic_to(
            (left, bottom - 5.0),
            (left - TAIL * 0.35, bottom - 0.8),
            (left - TAIL, bottom - 0.4),
        )
        .quad_to((left - TAIL - 0.6, bottom), (left - TAIL + 0.6, bottom))
        .line_to((left + 1.0, bottom))
        .close();
    let tail = tail.detach();
    body.op(&tail, PathOp::Union).unwrap_or(body)
}

/// 画成贴纸，返回 WebP（最长边 512）
pub fn render(quote: &Quote, theme: Theme) -> Result<Vec<u8>, BotError> {
    let image = render_rgba(quote, theme)?;
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::WebP)?;
    Ok(out.into_inner())
}

pub fn render_rgba(quote: &Quote, theme: Theme) -> Result<image::RgbaImage, BotError> {
    FONTS.with(|fonts| draw(&fonts.borrow(), quote, theme))
}

/// 昵称行：昵称在左，标签/头衔贴右
struct Header {
    name: Paragraph,
    /// (是不是彩色标签, 文字, 占的宽度)
    badge: Option<(bool, Paragraph, f32)>,
    name_width: f32,
    badge_space: f32,
}

impl Header {
    fn new(fonts: &FontCollection, quote: &Quote, palette: &Palette) -> Self {
        let name_color = palette.names[quote.color_index % palette.names.len()];
        let name_style = text_style(NAME_SIZE, name_color, Weight::SEMI_BOLD);
        let mut name = paragraph(fonts, &quote.name, &name_style, true, TextAlign::Left);
        name.layout(f32::MAX);
        let name_width = name.max_intrinsic_width().ceil();

        let badge = quote.badge.as_ref().map(|badge| {
            let (text, style) = match badge {
                Badge::Tag(text) => (text, text_style(TAG_SIZE, palette.tag, Weight::MEDIUM)),
                Badge::Title(text) => (text, text_style(META_SIZE, palette.meta, Weight::NORMAL)),
            };
            let mut paragraph = paragraph(fonts, text, &style, true, TextAlign::Left);
            paragraph.layout(MAX_CONTENT / 2.0);
            let text_width = paragraph
                .max_intrinsic_width()
                .ceil()
                .min(MAX_CONTENT / 2.0);
            paragraph.layout(text_width);
            let outer = match badge {
                Badge::Tag(_) => text_width + TAG_PAD_X * 2.0,
                Badge::Title(_) => text_width,
            };
            (matches!(badge, Badge::Tag(_)), paragraph, outer)
        });
        let badge_space = badge
            .as_ref()
            .map_or(0.0, |(_, _, width)| BADGE_GAP + width);
        Self {
            name,
            badge,
            name_width,
            badge_space,
        }
    }

    /// 按可用宽度 `content` 排好昵称，返回行高
    fn layout(&mut self, content: f32) -> f32 {
        self.name
            .layout((content - self.badge_space).max(MIN_CONTENT / 2.0));
        self.name.height()
    }

    fn paint(&mut self, canvas: &Canvas, left: f32, right: f32, top: f32, palette: &Palette) {
        let height = self.name.height();
        self.name.paint(canvas, (left, top));
        if let Some((is_tag, badge, outer)) = self.badge.as_mut() {
            let x = right - *outer;
            if *is_tag {
                let center = top + height / 2.0;
                let pill = Rect::from_xywh(x, center - TAG_HEIGHT / 2.0, *outer, TAG_HEIGHT);
                let mut paint = Paint::default();
                paint
                    .set_anti_alias(true)
                    .set_color(palette.tag)
                    .set_alpha_f(palette.tag_alpha);
                canvas.draw_rrect(
                    RRect::new_rect_xy(pill, TAG_HEIGHT / 2.0, TAG_HEIGHT / 2.0),
                    &paint,
                );
                badge.paint(canvas, (x + TAG_PAD_X, center - badge.height() / 2.0));
            } else {
                let baseline = top + self.name.alphabetic_baseline();
                badge.paint(canvas, (x, baseline - badge.alphabetic_baseline()));
            }
        }
    }
}

/// jpg/png/webp 都交给 image 解码（skia 没开 webp），转成 skia 的位图
fn decode_media(bytes: &[u8]) -> Option<Image> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let row_bytes = width as usize * 4;
    images::raster_from_data(&info, Data::new_copy(rgba.as_raw()), row_bytes)
}

/// 等比铺满 `dst`，多出来的部分居中裁掉
fn draw_cover(canvas: &Canvas, image: &Image, dst: Rect) {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let scale = (dst.width() / width).max(dst.height() / height);
    let (src_width, src_height) = (dst.width() / scale, dst.height() / scale);
    let src = Rect::from_xywh(
        (width - src_width) / 2.0,
        (height - src_height) / 2.0,
        src_width,
        src_height,
    );
    let image = image
        .with_default_mipmaps()
        .unwrap_or_else(|| image.clone());
    canvas.draw_image_rect_with_sampling_options(
        &image,
        Some((&src, skia_safe::canvas::SrcRectConstraint::Fast)),
        dst,
        SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
        &Paint::default(),
    );
}

/// 图片、贴纸上的时间：半透明黑底白字的小胶囊，右下角对齐 `(right, bottom)`
fn draw_time_pill(canvas: &Canvas, fonts: &FontCollection, time: &str, right: f32, bottom: f32) {
    if time.is_empty() {
        return;
    }
    let style = text_style(META_SIZE - 1.0, Color::WHITE, Weight::NORMAL);
    let mut text = paragraph(fonts, time, &style, true, TextAlign::Left);
    text.layout(f32::MAX);
    let width = text.max_intrinsic_width().ceil() + TIME_PILL_PAD_X * 2.0;
    let pill = Rect::from_xywh(
        right - width,
        bottom - TIME_PILL_HEIGHT,
        width,
        TIME_PILL_HEIGHT,
    );
    let mut paint = Paint::default();
    paint
        .set_anti_alias(true)
        .set_color(Color::from_argb(0x80, 0, 0, 0));
    canvas.draw_rrect(
        RRect::new_rect_xy(pill, TIME_PILL_HEIGHT / 2.0, TIME_PILL_HEIGHT / 2.0),
        &paint,
    );
    text.paint(
        canvas,
        (
            pill.left + TIME_PILL_PAD_X,
            pill.center_y() - text.height() / 2.0,
        ),
    );
}

fn draw_shadow(canvas: &Canvas, path: &skia_safe::Path, palette: &Palette) {
    if let Some(shadow) = palette.shadow {
        let mut paint = Paint::default();
        paint
            .set_anti_alias(true)
            .set_color(shadow)
            .set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 1.2, None));
        canvas.save();
        canvas.translate((0.0, 0.6));
        canvas.draw_path(path, &paint);
        canvas.restore();
    }
}

fn draw(fonts: &FontCollection, quote: &Quote, theme: Theme) -> Result<image::RgbaImage, BotError> {
    let palette = Palette::of(theme);
    let media = quote
        .media
        .as_ref()
        .and_then(|media| decode_media(&media.bytes).map(|image| (media.kind, image)));
    match media {
        Some((MediaKind::Sticker, sticker)) => draw_sticker(fonts, quote, &palette, &sticker),
        Some((MediaKind::Photo, photo)) => draw_bubble(fonts, quote, &palette, Some(&photo)),
        None => draw_bubble(fonts, quote, &palette, None),
    }
}

/// 文字和图片消息：头像 + 带尾巴的气泡，图片铺满气泡宽度，说明文字在图片下面
fn draw_bubble(
    fonts: &FontCollection,
    quote: &Quote,
    palette: &Palette,
    photo: Option<&Image>,
) -> Result<image::RgbaImage, BotError> {
    let mut header = Header::new(fonts, quote, palette);
    // 图片消息可以没有说明文字，这时时间压在图片上
    let has_text = photo.is_none() || !quote.text.trim().is_empty();

    let mut body = body_paragraph(fonts, quote, palette);
    body.layout(MAX_CONTENT);
    let body_width = match has_text {
        true => body.max_intrinsic_width().ceil().min(MAX_CONTENT),
        false => 0.0,
    };

    let mut time = paragraph(
        fonts,
        &quote.time,
        &text_style(META_SIZE, palette.meta, Weight::NORMAL),
        true,
        TextAlign::Left,
    );
    time.layout(f32::MAX);
    let time_width = time.max_intrinsic_width().ceil();

    let mut content = (header.name_width + header.badge_space)
        .max(body_width)
        .clamp(MIN_CONTENT, MAX_CONTENT);
    // 图片按比例定宽：太高的图收窄，最窄也有 PHOTO_MIN_WIDTH
    let aspect = photo.map(|photo| photo.height() as f32 / photo.width() as f32);
    if let Some(aspect) = aspect {
        let natural = (PHOTO_MAX_HEIGHT / aspect).clamp(PHOTO_MIN_WIDTH, PHOTO_MAX_WIDTH);
        content = content.max(natural - PAD_X * 2.0);
    }
    body.layout(content);
    let last_line = body
        .get_line_metrics()
        .last()
        .map(|line| (line.width as f32, line.baseline as f32));
    let last_width = last_line.map_or(0.0, |(width, _)| width);
    // 时间尽量挤在最后一行右边，放不下才另起一行
    let time_inline = if !has_text || last_width + TIME_GAP + time_width <= content {
        true
    } else if last_width + TIME_GAP + time_width <= MAX_CONTENT {
        content = (last_width + TIME_GAP + time_width).ceil();
        body.layout(content);
        true
    } else {
        false
    };
    let header_height = header.layout(content);
    let bubble_width = content + PAD_X * 2.0;

    let photo_height = aspect.map_or(0.0, |aspect| (bubble_width * aspect).min(PHOTO_MAX_HEIGHT));
    let (photo_top, body_top, bubble_height) = match photo {
        Some(_) => {
            let photo_top = PAD_TOP + header_height + PHOTO_GAP;
            let photo_bottom = photo_top + photo_height;
            match has_text {
                true => {
                    let body_top = photo_bottom + CAPTION_GAP;
                    let mut bottom = body_top + body.height() + PAD_BOTTOM;
                    if !time_inline {
                        bottom += time.height();
                    }
                    (photo_top, body_top, bottom)
                }
                // 没有说明文字时图片贴着气泡底边
                false => (photo_top, photo_bottom, photo_bottom),
            }
        }
        None => {
            let body_top = PAD_TOP + header_height + NAME_TEXT_GAP;
            let mut bottom = body_top + body.height() + PAD_BOTTOM;
            if !time_inline {
                bottom += time.height();
            }
            (0.0, body_top, bottom)
        }
    };

    let bubble_x = MARGIN + AVATAR + AVATAR_GAP;
    let bubble_y = MARGIN + (AVATAR - bubble_height).max(0.0);
    let width = bubble_x + bubble_width + MARGIN;
    let height = MARGIN * 2.0 + bubble_height.max(AVATAR);

    rasterize(width, height, |canvas| {
        let bubble = Rect::from_xywh(bubble_x, bubble_y, bubble_width, bubble_height);
        let path = bubble_path(bubble);
        draw_shadow(canvas, &path, palette);
        let mut fill = Paint::default();
        fill.set_anti_alias(true).set_color(palette.bubble);
        canvas.draw_path(&path, &fill);

        // 头像和气泡底边对齐
        let avatar = Rect::from_xywh(MARGIN, bubble.bottom - AVATAR, AVATAR, AVATAR);
        draw_avatar(canvas, fonts, avatar, quote);

        header.paint(
            canvas,
            bubble.left + PAD_X,
            bubble.right - PAD_X,
            bubble.top + PAD_TOP,
            palette,
        );

        if let Some(photo) = photo {
            // 裁进气泡的形状，贴底时下面两个角跟着圆
            let rect = Rect::from_xywh(
                bubble.left,
                bubble.top + photo_top,
                bubble_width,
                photo_height,
            );
            canvas.save();
            canvas.clip_path(&path, ClipOp::Intersect, true);
            draw_cover(canvas, photo, rect);
            canvas.restore();
            if !has_text {
                draw_time_pill(
                    canvas,
                    fonts,
                    &quote.time,
                    rect.right - TIME_PILL_INSET,
                    rect.bottom - TIME_PILL_INSET,
                );
                return;
            }
        }

        let body_y = bubble.top + body_top;
        body.paint(canvas, (bubble.left + PAD_X, body_y));
        let time_x = bubble.right - PAD_X - time_width;
        let time_y = match (time_inline, last_line) {
            (true, Some((_, baseline))) => body_y + baseline - time.alphabetic_baseline() + 1.5,
            _ => body_y + body.height(),
        };
        time.paint(canvas, (time_x, time_y));
    })
}

/// 贴纸消息和 Telegram 里一样不带气泡：昵称放在上方的小胶囊里，时间压在贴纸右下角
fn draw_sticker(
    fonts: &FontCollection,
    quote: &Quote,
    palette: &Palette,
    sticker: &Image,
) -> Result<image::RgbaImage, BotError> {
    let mut header = Header::new(fonts, quote, palette);
    let scale = STICKER_BOX / sticker.width().max(sticker.height()) as f32;
    let sticker_width = sticker.width() as f32 * scale;
    let sticker_height = sticker.height() as f32 * scale;

    let content = (header.name_width + header.badge_space).clamp(MIN_CONTENT / 2.0, MAX_CONTENT);
    let header_height = header.layout(content);
    let pill_width = content + NAME_PILL_PAD_X * 2.0;
    let pill_height = header_height + NAME_PILL_PAD_Y * 2.0;

    let left = MARGIN + AVATAR + AVATAR_GAP;
    let column = pill_height + NAME_PILL_GAP + sticker_height;
    let top = MARGIN + (AVATAR - column).max(0.0);
    let width = left + sticker_width.max(pill_width) + MARGIN;
    let height = MARGIN * 2.0 + column.max(AVATAR);

    rasterize(width, height, |canvas| {
        let pill = Rect::from_xywh(left, top, pill_width, pill_height);
        let radius = (pill_height / 2.0).min(RADIUS);
        let mut path = PathBuilder::new();
        path.add_rrect(RRect::new_rect_xy(pill, radius, radius), None, None);
        let path = path.detach();
        draw_shadow(canvas, &path, palette);
        let mut fill = Paint::default();
        fill.set_anti_alias(true).set_color(palette.bubble);
        canvas.draw_path(&path, &fill);
        header.paint(
            canvas,
            pill.left + NAME_PILL_PAD_X,
            pill.right - NAME_PILL_PAD_X,
            pill.top + NAME_PILL_PAD_Y,
            palette,
        );

        let rect = Rect::from_xywh(
            left,
            pill.bottom + NAME_PILL_GAP,
            sticker_width,
            sticker_height,
        );
        draw_cover(canvas, sticker, rect);
        draw_time_pill(
            canvas,
            fonts,
            &quote.time,
            rect.right - TIME_PILL_INSET,
            rect.bottom - TIME_PILL_INSET,
        );

        let avatar = Rect::from_xywh(MARGIN, rect.bottom - AVATAR, AVATAR, AVATAR);
        draw_avatar(canvas, fonts, avatar, quote);
    })
}

/// 按逻辑尺寸 `width × height` 画，最长边缩放到 STICKER_SIDE 像素
fn rasterize(
    width: f32,
    height: f32,
    paint: impl FnOnce(&Canvas),
) -> Result<image::RgbaImage, BotError> {
    let scale = STICKER_SIDE / width.max(height);
    let (px_width, px_height) = if width >= height {
        (
            STICKER_SIDE as i32,
            (height * scale).round().max(1.0) as i32,
        )
    } else {
        ((width * scale).round().max(1.0) as i32, STICKER_SIDE as i32)
    };

    let mut surface = surfaces::raster_n32_premul((px_width, px_height))
        .ok_or_else(|| BotError::Custom("创建画布失败".to_string()))?;
    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);
    canvas.scale((scale, scale));
    paint(canvas);

    let info = ImageInfo::new(
        (px_width, px_height),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let row_bytes = px_width as usize * 4;
    let mut pixels = vec![0u8; row_bytes * px_height as usize];
    if !surface.read_pixels(&info, &mut pixels, row_bytes, (0, 0)) {
        return Err(BotError::Custom("读取画布失败".to_string()));
    }
    image::RgbaImage::from_raw(px_width as u32, px_height as u32, pixels)
        .ok_or_else(|| BotError::Custom("读取画布失败".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(text: &str, badge: Option<Badge>) -> Quote {
        Quote {
            name: "蘑菇蘑菇蘑 | 没回就是在打游戏".to_string(),
            color_index: 6,
            badge,
            avatar: None,
            text: text.to_string(),
            spans: Vec::new(),
            time: "19:22".to_string(),
            media: None,
        }
    }

    #[test]
    fn stickers_have_a_512_side() {
        let short = render_rgba(
            &sample("BV1g2xW6JEEz", Some(Badge::Tag("GoGo新天地".into()))),
            Theme::Light,
        )
        .unwrap();
        assert_eq!(short.width(), 512);
        assert!(short.height() <= 512);

        let long = render_rgba(&sample(&"很长的一段话".repeat(80), None), Theme::Dark).unwrap();
        assert_eq!(long.height(), 512);
        assert!(long.width() <= 512);
    }

    #[test]
    fn spans_out_of_bounds_are_ignored() {
        let mut quote = sample("你好 world", None);
        quote.spans = vec![
            Span {
                range: 0..3,
                style: SpanStyle {
                    bold: true,
                    ..Default::default()
                },
            },
            Span {
                range: 1..2,
                style: SpanStyle {
                    link: true,
                    ..Default::default()
                },
            },
            Span {
                range: 7..99,
                style: SpanStyle {
                    italic: true,
                    ..Default::default()
                },
            },
        ];
        assert!(render(&quote, Theme::Dark).is_ok());
    }

    /// 模拟没装字体的服务器：只用仓库字体，中文、各字重和 emoji 都得有字形。
    /// 自带的是位图 emoji（CBDT），只有 Linux 的 FreeType 认，mac 上由系统 emoji 兜底
    #[test]
    fn bundled_fonts_cover_text_and_emoji() {
        let mut fonts = FontCollection::new();
        fonts.set_asset_font_manager(Some(bundled_fonts().expect("data/fonts 里应有字体").into()));
        fonts.disable_font_fallback();
        for weight in [
            Weight::NORMAL,
            Weight::MEDIUM,
            Weight::SEMI_BOLD,
            Weight::BOLD,
        ] {
            let style = text_style(TEXT_SIZE, Color::BLACK, weight);
            let sample = if cfg!(target_os = "linux") {
                "蘑菇 GoGo新天地，你好！😀👍🏻🇨🇳"
            } else {
                "蘑菇 GoGo新天地，你好！"
            };
            let mut text = paragraph(&fonts, sample, &style, false, TextAlign::Left);
            text.layout(MAX_CONTENT);
            assert_eq!(text.unresolved_glyphs(), Some(0), "{weight:?}");
        }
        if let Ok(dir) = std::env::var("QUOTE_PREVIEW") {
            let mut quote = sample(
                "表情 😀👍🏻🇨🇳 和 **粗体**",
                Some(Badge::Tag("GoGo新天地".into())),
            );
            quote.spans = vec![Span {
                range: quote.text.find("**").unwrap()..quote.text.len(),
                style: SpanStyle {
                    bold: true,
                    ..Default::default()
                },
            }];
            draw(&fonts, &quote, Theme::Light)
                .unwrap()
                .save(Path::new(&dir).join("bundled.png"))
                .unwrap();
        }
    }

    /// 测试用的渐变图，带一个透明的角，按 `format` 编码
    fn test_image(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            let alpha = if x < width / 4 && y < height / 4 {
                0
            } else {
                255
            };
            image::Rgba([
                (x * 255 / width) as u8,
                (y * 255 / height) as u8,
                160,
                alpha,
            ])
        });
        let mut out = Cursor::new(Vec::new());
        match format {
            image::ImageFormat::Jpeg => image::DynamicImage::from(image)
                .into_rgb8()
                .write_to(&mut out, format),
            _ => image.write_to(&mut out, format),
        }
        .unwrap();
        out.into_inner()
    }

    fn media_samples() -> Vec<(&'static str, Quote)> {
        let photo = |width, height, text: &str| {
            let mut quote = sample(text, Some(Badge::Title("管理员".into())));
            quote.media = Some(Media {
                kind: MediaKind::Photo,
                bytes: test_image(width, height, image::ImageFormat::Jpeg),
            });
            quote
        };
        let mut sticker = sample("", Some(Badge::Tag("GoGo新天地".into())));
        sticker.media = Some(Media {
            kind: MediaKind::Sticker,
            bytes: test_image(512, 400, image::ImageFormat::WebP),
        });
        vec![
            ("photo.png", photo(800, 600, "")),
            ("photo_tall.png", photo(400, 1200, "")),
            (
                "photo_caption.png",
                photo(
                    1280,
                    720,
                    "这张图的说明文字，稍微长一点看看换行和时间的位置",
                ),
            ),
            ("sticker.png", sticker),
        ]
    }

    #[test]
    fn media_quotes_fit_a_sticker() {
        for (name, quote) in media_samples() {
            for theme in [Theme::Light, Theme::Dark] {
                let image = render_rgba(&quote, theme).unwrap();
                assert_eq!(image.width().max(image.height()), 512, "{name}");
            }
        }
        // 解不开的图当作没有，仍按文字画
        let mut broken = sample("坏图", None);
        broken.media = Some(Media {
            kind: MediaKind::Photo,
            bytes: b"not an image".to_vec(),
        });
        assert!(render(&broken, Theme::Dark).is_ok());
    }

    #[test]
    fn initials_follow_telegram() {
        assert_eq!(initials("蘑菇蘑菇蘑 | 没回就是在打游戏"), "蘑");
        assert_eq!(initials("john smith"), "JS");
        assert_eq!(initials("Allen"), "A");
        assert_eq!(initials(""), "");
    }

    /// `QUOTE_PREVIEW=目录 cargo test quote::tests::preview -- --ignored` 输出样张
    #[test]
    #[ignore]
    fn preview() {
        let dir = std::env::var("QUOTE_PREVIEW").unwrap_or_else(|_| ".".to_string());
        let avatar = std::env::var("QUOTE_AVATAR")
            .ok()
            .and_then(|path| std::fs::read(path).ok());
        let mut quote = sample("BV1g2xW6JEEz", Some(Badge::Tag("GoGo新天地".into())));
        quote.avatar = avatar;
        for (theme, file) in [(Theme::Light, "light.png"), (Theme::Dark, "dark.png")] {
            render_rgba(&quote, theme)
                .unwrap()
                .save(Path::new(&dir).join(file))
                .unwrap();
        }
        let text = "今天天气不错 🌞，去 https://example.com 看看吧，顺便测试一下很长很长的一段文字会不会正确换行，以及时间戳的位置。";
        let range = |part: &str| {
            let start = text.find(part).unwrap();
            start..start + part.len()
        };
        let mut long = sample(text, Some(Badge::Title("管理员".into())));
        long.color_index = 2;
        long.spans = vec![
            Span {
                range: range("天气"),
                style: SpanStyle {
                    bold: true,
                    ..Default::default()
                },
            },
            Span {
                range: range("https://example.com"),
                style: SpanStyle {
                    link: true,
                    ..Default::default()
                },
            },
        ];
        render_rgba(&long, Theme::Dark)
            .unwrap()
            .save(Path::new(&dir).join("long.png"))
            .unwrap();
        for (file, quote) in media_samples() {
            render_rgba(&quote, Theme::Dark)
                .unwrap()
                .save(Path::new(&dir).join(file))
                .unwrap();
        }
    }
}
