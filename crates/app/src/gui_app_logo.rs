//! Full-color logo rendered at the window's actual pixel density.

use gpui_kit::{prelude::*, *};
use std::sync::Arc;

struct Logo;

impl Asset for Logo {
    type Source = i32;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        side: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        async move {
            let svg = renderer.parse_svg(include_bytes!("../assets/app_logo.svg"))?;
            // The generic SVG image loader rasterizes at its intrinsic size and
            // downsamples straight-alpha pixels, leaving a dark fringe at clear edges.
            renderer
                .render_parsed(
                    &svg,
                    SvgSize::ExactSize(size(DevicePixels(side), DevicePixels(side))),
                )
                .map_err(Into::into)
        }
    }
}

pub(super) fn app_logo(side: f32, window: &Window) -> Stateful<Div> {
    let pixels = (side * window.scale_factor()).round().max(1.) as i32;
    let side = pixels as f32 / window.scale_factor();
    div()
        .id("about-app-logo")
        .role(accesskit::Role::Image)
        .aria_label(app::APP_NAME)
        .relative()
        .size(px(side))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, cx| {
                    // Match the supplied SVG's inset rectangle and radius in its
                    // 256-unit viewBox, rather than the square image bounds.
                    let unit = side / 256.;
                    let silhouette = Bounds::new(
                        bounds.origin + point(px(0.5 * unit), px(0.5 * unit)),
                        size(px(255. * unit), px(255. * unit)),
                    );
                    window.paint_drop_shadows(
                        silhouette,
                        Corners::all(px(55.55 * unit)),
                        &crate::gui_style::card_shadow(cx),
                    );
                },
            )
            .absolute()
            .size_full(),
        )
        .child(
            img(move |window: &mut Window, cx: &mut App| window.use_asset::<Logo>(&pixels, cx))
                .size(px(side))
                .object_fit(ObjectFit::Contain),
        )
}
