use crate::{config::Config, x11_window::X11Window};
use anyhow::{Context as _, Result, bail};
use egui::Color32;
use egui_glow::Painter;
use egui_glow::glow::Context as GlowContext;
use glutin::{
    config::ConfigTemplateBuilder,
    context::{ContextApi, ContextAttributesBuilder, NotCurrentContext, PossiblyCurrentContext},
    display::Display,
    prelude::{GlDisplay as _, NotCurrentGlContext, PossiblyCurrentGlContext},
    surface::{GlSurface as _, Surface, SurfaceAttributesBuilder, WindowSurface},
};
use log::{info, trace, warn};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle, XlibDisplayHandle,
};
use std::{
    ffi::{CString, c_int},
    num::{NonZero, NonZeroU32},
    ptr::NonNull,
    sync::Arc,
};

pub struct OpenGLContext<'a> {
    pub dimensions: [u32; 2],
    pub background: (f32, f32, f32, f32),
    pub painter: Painter,
    window: &'a X11Window<'a>,
    display: Display,
    config: glutin::config::Config,
    surface: Surface<WindowSurface>,
    context: Option<PossiblyCurrentContext>,
    gl: Arc<GlowContext>,
}

impl<'a> OpenGLContext<'a> {
    pub fn new(window: &'a X11Window, config: &Config) -> Result<Self> {
        info!("creating GL display via EGL");

        let background_color: Color32 = config.theme.background.into();
        let (r, g, b, a) = background_color.to_tuple();
        let dimensions = [
            config.layout.window_dimensions.width as _,
            config.layout.window_dimensions.height as _,
        ];

        // NVIDIA's EGL does not implement EGL_EXT_platform_xcb, so passing an XCB handle
        // causes libglvnd to skip NVIDIA and fall back to Mesa, which fails with
        // "egl: failed to create dri2 screen". We open a throwaway Xlib display instead
        // since NVIDIA only supports EGL_PLATFORM_X11_KHR with a raw Xlib Display*.
        // All actual X11 work is still done via XCB.
        let display_handle = get_xlib_display_handle(window.screen_num as _)
            .inspect_err(|e| warn!("failed to get Xlib display handle, fallback to XCB: {}", e))
            .map(RawDisplayHandle::Xlib)
            .unwrap_or_else(|_| {
                RawDisplayHandle::Xcb(XcbDisplayHandle::new(
                    NonNull::new(window.conn.get_raw_xcb_connection()),
                    window.screen_num as _,
                ))
            });

        let gl_display =
            unsafe { Display::new(display_handle, glutin::display::DisplayApiPreference::Egl)? };

        let config_template_builder = ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(true);

        let display_config = unsafe {
            gl_display
                .find_configs(config_template_builder.build())?
                .next()
                .context("No suitable config found")?
        };

        let attrs = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(glutin::context::Version::new(
                3, 3,
            ))))
            .build(None);
        let context = unsafe { gl_display.create_context(&display_config, &attrs)? };

        info!("creating egui_glow painter");
        let (painter, surface, context, gl) = Self::create_painter(
            window.win_id.get(),
            &gl_display,
            &display_config,
            context,
            dimensions,
        )?;

        Ok(OpenGLContext {
            window,
            display: gl_display,
            config: display_config,
            dimensions,
            background: (
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ),
            surface,
            context: Some(context),
            gl,
            painter,
        })
    }

    pub fn recreate_painter(&mut self) -> Result<()> {
        info!("recreating egui_glow painter");

        self.painter.destroy();
        let not_current_ctx = self.context.take().unwrap().make_not_current()?;

        let (painter, surface, context, gl) = Self::create_painter(
            self.window.win_id.get(),
            &self.display,
            &self.config,
            not_current_ctx,
            self.dimensions,
        )?;

        self.painter = painter;
        self.surface = surface;
        self.context = Some(context);
        self.gl = gl;

        Ok(())
    }

    fn create_painter(
        win_id: u32,
        gl_display: &Display,
        display_config: &glutin::config::Config,
        ctx: NotCurrentContext,
        dimensions: [u32; 2],
    ) -> Result<(
        Painter,
        Surface<WindowSurface>,
        PossiblyCurrentContext,
        Arc<GlowContext>,
    )> {
        let window_handle = XcbWindowHandle::new(NonZero::new(win_id).unwrap());

        let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            RawWindowHandle::Xcb(window_handle),
            NonZero::new(dimensions[0]).unwrap(),
            NonZero::new(dimensions[1]).unwrap(),
        );
        let surface = unsafe { gl_display.create_window_surface(display_config, &surface_attrs)? };
        let ctx = ctx.make_current(&surface)?;

        surface.set_swap_interval(&ctx, glutin::surface::SwapInterval::Wait(NonZeroU32::MIN))?;

        let gl = unsafe {
            GlowContext::from_loader_function(|s| {
                gl_display.get_proc_address(CString::new(s).unwrap().as_c_str())
            })
        };
        let gl = Arc::new(gl);

        let painter = Painter::new(gl.clone(), "", None, true)?;

        Ok((painter, surface, ctx, gl))
    }

    pub fn render(
        &mut self,
        egui_ctx: &egui::Context,
        full_output: egui::FullOutput,
    ) -> Result<()> {
        trace!("rendering frame");
        let egui::FullOutput {
            platform_output: _,
            mut textures_delta,
            mut shapes,
            pixels_per_point,
            viewport_output: _,
        } = full_output;

        let (r, g, b, a) = self.background;
        self.painter.clear(self.dimensions, [r, g, b, a]);

        let shapes = std::mem::take(&mut shapes);
        let clipped_primitives = egui_ctx.tessellate(shapes, pixels_per_point);
        self.painter.paint_and_update_textures(
            self.dimensions,
            pixels_per_point,
            &clipped_primitives,
            &mut textures_delta,
        );

        self.surface.swap_buffers(self.context.as_ref().unwrap())?;
        trace!("frame rendered");

        Ok(())
    }

    pub fn destroy(&mut self) {
        info!("destroying painter");
        self.painter.destroy();
    }
}

fn get_xlib_display_handle(screen: c_int) -> Result<XlibDisplayHandle> {
    unsafe {
        let lib = libloading::Library::new("libX11.so.6")?;
        let open_display_func: libloading::Symbol<
            unsafe extern "C" fn(*const i8) -> *mut std::ffi::c_void,
        > = lib.get(b"XOpenDisplay")?;

        let dpy = open_display_func(std::ptr::null());
        if dpy.is_null() {
            bail!("failed to open X11 display");
        }

        std::mem::forget(lib); // keep lib alive
        Ok(XlibDisplayHandle::new(NonNull::new(dpy), screen))
    }
}
