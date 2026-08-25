use crate::{
    backend::render::element::AsGlowRenderer,
    state::State,
    utils::{
        iced::{IcedElementInternal, IcedRenderElement},
        prelude::*,
    },
};
use calloop::LoopHandle;
use LINGMO_comp_config::AppearanceConfig;
use id_tree::NodeId;
use smithay::{
    backend::{
        drm::DrmNode,
        input::KeyState,
        renderer::{
            element::{
                Element, Kind, RenderElement, UnderlyingStorage,
                utils::{CropRenderElement, RelocateRenderElement, RescaleRenderElement},
            },
            gles::element::PixelShaderElement,
            glow::GlowRenderer,
            utils::{DamageSet, OpaqueRegions},
        },
    },
    desktop::{WindowSurfaceType, space::SpaceElement},
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    },
    output::Output,
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface},
    space_elements,
    utils::{
        Buffer as BufferCoords, IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial, Size,
        user_data::UserDataMap,
    },
    wayland::seat::WaylandFocus,
    xwayland::{X11Surface, xwm::X11Relatable},
};
use stack::LINGMOStackInternal;
use window::LINGMOWindowInternal;

use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::{Arc, Mutex, Weak, atomic::AtomicBool},
};

pub mod surface;
use self::stack::MoveResult;
pub use self::surface::LINGMOSurface;
pub mod stack;
pub use self::stack::LINGMOStack;
pub mod window;
pub use self::window::LINGMOWindow;
pub mod resize_indicator;
pub mod stack_hover;
pub mod swap_indicator;

#[cfg(feature = "debug")]
use egui_plot::{Corner, Legend, Plot, PlotPoints, Polygon};
#[cfg(feature = "debug")]
use smithay::backend::renderer::{element::texture::TextureRenderElement, gles::GlesTexture};
#[cfg(feature = "debug")]
use smithay::desktop::WindowSurface;
#[cfg(feature = "debug")]
use tracing::debug;

use super::{
    ManagedLayer,
    focus::target::PointerFocusTarget,
    layout::{
        floating::{ResizeState, TiledCorners},
        tiling::NodeDesc,
    },
};
use LINGMO_settings_config::shortcuts::action::{Direction, FocusDirection};

space_elements! {
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    LINGMOMappedInternal;
    Window=LINGMOWindow,
    Stack=LINGMOStack,
}

#[derive(Debug, Clone, Copy)]
pub struct MaximizedState {
    pub original_geometry: Rectangle<i32, Local>,
    pub original_layer: ManagedLayer,
    pub original_snapped: Option<TiledCorners>,
}

#[derive(Clone)]
pub struct LINGMOMapped {
    element: LINGMOMappedInternal,

    // associated data
    pub maximized_state: Arc<Mutex<Option<MaximizedState>>>,

    //tiling
    pub tiling_node_id: Arc<Mutex<Option<NodeId>>>,
    //floating
    pub(super) resize_state: Arc<Mutex<Option<ResizeState>>>,
    pub last_geometry: Arc<Mutex<Option<Rectangle<i32, Local>>>>,
    pub moved_since_mapped: Arc<AtomicBool>,
    pub floating_tiled: Arc<Mutex<Option<TiledCorners>>>,
    //sticky
    pub previous_layer: Arc<Mutex<Option<ManagedLayer>>>,

    #[cfg(feature = "debug")]
    debug: Arc<Mutex<Option<smithay_egui::EguiState>>>,
}

impl fmt::Debug for LINGMOMapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LINGMOMapped")
            .field("element", &self.element)
            .field("maximized_state", &self.maximized_state)
            .field("tiling_node_id", &self.tiling_node_id)
            .field("resize_state", &self.resize_state)
            .field("last_geometry", &self.last_geometry)
            .field("moved_since_mapped", &self.moved_since_mapped)
            .field("floating_tiled", &self.floating_tiled)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct LINGMOMappedKey(LINGMOMappedKeyInner);
#[derive(Clone, Debug)]
enum LINGMOMappedKeyInner {
    Window(Weak<Mutex<IcedElementInternal<LINGMOWindowInternal>>>),
    Stack(Weak<Mutex<IcedElementInternal<LINGMOStackInternal>>>),
}

impl Hash for LINGMOMappedKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.0 {
            LINGMOMappedKeyInner::Window(weak) => weak.as_ptr().hash(state),
            LINGMOMappedKeyInner::Stack(weak) => weak.as_ptr().hash(state),
        }
    }
}

impl IsAlive for LINGMOMappedKey {
    fn alive(&self) -> bool {
        match &self.0 {
            LINGMOMappedKeyInner::Window(weak) => weak.strong_count() > 0,
            LINGMOMappedKeyInner::Stack(weak) => weak.strong_count() > 0,
        }
    }
}

impl PartialEq for LINGMOMappedKey {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (LINGMOMappedKeyInner::Window(weak1), LINGMOMappedKeyInner::Window(weak2)) => {
                Weak::ptr_eq(weak1, weak2)
            }
            (LINGMOMappedKeyInner::Stack(weak1), LINGMOMappedKeyInner::Stack(weak2)) => {
                Weak::ptr_eq(weak1, weak2)
            }
            _ => false,
        }
    }
}
impl Eq for LINGMOMappedKey {}

impl PartialEq<LINGMOMappedKey> for LINGMOMapped {
    fn eq(&self, other: &LINGMOMappedKey) -> bool {
        match (&self.element, &other.0) {
            (LINGMOMappedInternal::Window(window), LINGMOMappedKeyInner::Window(weak)) => {
                Arc::as_ptr(&window.0.0) == weak.as_ptr()
            }
            (LINGMOMappedInternal::Stack(stack), LINGMOMappedKeyInner::Stack(weak)) => {
                Arc::as_ptr(&stack.0.0) == weak.as_ptr()
            }
            _ => false,
        }
    }
}

impl PartialEq for LINGMOMapped {
    fn eq(&self, other: &Self) -> bool {
        self.element == other.element
    }
}

impl Eq for LINGMOMapped {}

impl Hash for LINGMOMapped {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.element.hash(state)
    }
}

impl LINGMOMapped {
    pub fn windows(&self) -> impl Iterator<Item = (LINGMOSurface, Point<i32, Logical>)> + '_ {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => {
                Box::new(stack.surfaces().map(|w| (w, stack.offset())))
                    as Box<dyn Iterator<Item = (LINGMOSurface, Point<i32, Logical>)>>
            }
            LINGMOMappedInternal::Window(window) => {
                Box::new(std::iter::once((window.surface(), window.offset())))
            }
            _ => Box::new(std::iter::empty()),
        }
    }

    pub fn active_window(&self) -> LINGMOSurface {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => stack.active(),
            LINGMOMappedInternal::Window(win) => win.surface(),
            _ => unreachable!(),
        }
    }

    pub fn has_active_window(&self, window: &LINGMOSurface) -> bool {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => stack.has_active(window),
            LINGMOMappedInternal::Window(win) => win.contains_surface(window),
            _ => unreachable!(),
        }
    }

    pub fn active_window_offset(&self) -> Point<i32, Logical> {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => stack.offset(),
            LINGMOMappedInternal::Window(window) => window.offset(),
            _ => unreachable!(),
        }
    }

    pub fn active_window_geometry(&self) -> Rectangle<i32, Logical> {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => {
                let win = stack.active();
                let location = stack.offset();
                let size = win.geometry().size;
                Rectangle::new(location, size)
            }
            LINGMOMappedInternal::Window(win) => {
                let location = win.offset();
                let size = win.geometry().size;
                Rectangle::new(location, size)
            }
            _ => unreachable!(),
        }
    }

    pub fn set_active<S>(&self, window: &S)
    where
        LINGMOSurface: PartialEq<S>,
    {
        if let LINGMOMappedInternal::Stack(stack) = &self.element {
            stack.set_active(window);
        }
    }

    pub fn focus_window(&self, window: &LINGMOSurface) {
        if let LINGMOMappedInternal::Stack(stack) = &self.element {
            stack.set_active(window)
        }
    }

    pub fn has_surface(&self, surface: &WlSurface, surface_type: WindowSurfaceType) -> bool {
        self.windows()
            .any(|(w, _)| w.has_surface(surface, surface_type))
    }

    pub fn surface_offset(&self, surface: &WlSurface) -> Option<Point<i32, Logical>> {
        self.windows().find_map(|(window, window_offset)| {
            window
                .surface_offset(surface)
                .map(|offset| window_offset + offset)
        })
    }

    /// Give the pointer target under a relative offset into this element.
    ///
    /// Returns Target + Offset relative to the target
    pub fn focus_under(
        &self,
        relative_pos: Point<f64, Logical>,
        surface_type: WindowSurfaceType,
        seat: &Seat<State>,
    ) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => stack.focus_under(relative_pos, surface_type),
            LINGMOMappedInternal::Window(window) => {
                window.focus_under(relative_pos, surface_type, Some(seat))
            }
            _ => unreachable!(),
        }
    }

    pub fn handle_move(&self, direction: Direction) -> MoveResult {
        if let LINGMOMappedInternal::Stack(stack) = &self.element {
            stack.handle_move(direction)
        } else {
            MoveResult::Default
        }
    }

    pub fn handle_focus(
        &self,
        seat: &Seat<State>,
        direction: FocusDirection,
        swap: Option<NodeDesc>,
    ) -> bool {
        if let LINGMOMappedInternal::Stack(stack) = &self.element {
            stack.handle_focus(seat, direction, swap)
        } else {
            false
        }
    }

    pub fn set_resizing(&self, resizing: bool) {
        for window in match &self.element {
            LINGMOMappedInternal::Stack(s) => {
                Box::new(s.surfaces()) as Box<dyn Iterator<Item = LINGMOSurface>>
            }
            LINGMOMappedInternal::Window(w) => Box::new(std::iter::once(w.surface())),
            _ => unreachable!(),
        } {
            window.set_resizing(resizing);
        }
    }

    pub fn is_resizing(&self, pending: bool) -> Option<bool> {
        let window = match &self.element {
            LINGMOMappedInternal::Stack(s) => s.active(),
            LINGMOMappedInternal::Window(w) => w.surface(),
            _ => unreachable!(),
        };

        window.is_resizing(pending)
    }

    pub fn set_tiled(&self, tiled: bool) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.set_tiled(tiled),
            LINGMOMappedInternal::Window(w) => w.set_tiled(tiled),
            _ => unreachable!(),
        }
    }

    pub fn is_tiled(&self, pending: bool) -> Option<bool> {
        let window = match &self.element {
            LINGMOMappedInternal::Stack(s) => s.active(),
            LINGMOMappedInternal::Window(w) => w.surface(),
            _ => unreachable!(),
        };

        window.is_tiled(pending)
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        for window in match &self.element {
            LINGMOMappedInternal::Stack(s) => {
                Box::new(s.surfaces()) as Box<dyn Iterator<Item = LINGMOSurface>>
            }
            LINGMOMappedInternal::Window(w) => Box::new(std::iter::once(w.surface())),
            _ => unreachable!(),
        } {
            window.set_fullscreen(fullscreen);
        }
    }

    pub fn is_fullscreen(&self, pending: bool) -> bool {
        let window = match &self.element {
            LINGMOMappedInternal::Stack(s) => s.active(),
            LINGMOMappedInternal::Window(w) => w.surface(),
            _ => unreachable!(),
        };

        window.is_fullscreen(pending)
    }

    pub fn set_maximized(&self, maximized: bool) {
        for window in match &self.element {
            LINGMOMappedInternal::Stack(s) => {
                Box::new(s.surfaces()) as Box<dyn Iterator<Item = LINGMOSurface>>
            }
            LINGMOMappedInternal::Window(w) => Box::new(std::iter::once(w.surface())),
            _ => unreachable!(),
        } {
            window.set_maximized(maximized)
        }
    }

    pub fn is_maximized(&self, pending: bool) -> bool {
        let window = match &self.element {
            LINGMOMappedInternal::Stack(s) => s.active(),
            LINGMOMappedInternal::Window(w) => w.surface(),
            _ => unreachable!(),
        };

        window.is_maximized(pending)
    }

    pub fn set_activated(&self, activated: bool) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.set_activate(activated),
            LINGMOMappedInternal::Window(w) => w.set_activate(activated),
            _ => unreachable!(),
        }
    }

    pub fn is_activated(&self, pending: bool) -> bool {
        let window = match &self.element {
            LINGMOMappedInternal::Stack(s) => s.active(),
            LINGMOMappedInternal::Window(w) => w.surface(),
            _ => unreachable!(),
        };

        window.is_activated(pending)
    }

    pub fn is_minimized(&self) -> bool {
        self.active_window().is_minimized()
    }

    pub fn set_minimized(&self, minimized: bool) {
        for (w, _) in self.windows() {
            w.set_minimized(minimized);
        }
    }

    pub fn pending_size(&self) -> Option<Size<i32, Logical>> {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.pending_size(),
            LINGMOMappedInternal::Window(w) => w.pending_size(),
            _ => unreachable!(),
        }
    }

    pub fn last_server_size(&self) -> Option<Size<i32, Logical>> {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.last_server_size(),
            LINGMOMappedInternal::Window(w) => w.last_server_size(),
            _ => unreachable!(),
        }
    }

    pub fn set_geometry(&self, geo: Rectangle<i32, Global>) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.set_geometry(geo),
            LINGMOMappedInternal::Window(w) => w.set_geometry(geo),
            _ => {}
        }
    }

    pub fn on_commit(&self, surface: &WlSurface) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.on_commit(surface),
            LINGMOMappedInternal::Window(w) => w.on_commit(surface),
            _ => {}
        }
    }

    pub fn min_size(&self) -> Option<Size<i32, Logical>> {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => stack.min_size(),
            LINGMOMappedInternal::Window(window) => window.min_size(),
            _ => unreachable!(),
        }
    }

    pub fn max_size(&self) -> Option<Size<i32, Logical>> {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => stack.max_size(),
            LINGMOMappedInternal::Window(window) => window.max_size(),
            _ => unreachable!(),
        }
    }

    pub fn set_bounds(&self, size: impl Into<Option<Size<i32, Logical>>>) {
        let size = size.into();
        for (surface, _) in self.windows() {
            surface.set_bounds(size)
        }
    }

    pub fn latest_size_committed(&self) -> bool {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.surfaces().any(|s| s.latest_size_committed()),
            LINGMOMappedInternal::Window(w) => w.surface().latest_size_committed(),
            _ => unreachable!(),
        }
    }

    pub fn configure(&self) -> Option<Serial> {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => {
                let active = s.active();
                for surface in s.surfaces().filter(|s| s != &active) {
                    surface.send_configure();
                }
                active.send_configure()
            }
            LINGMOMappedInternal::Window(w) => w.surface().send_configure(),
            _ => unreachable!(),
        }
    }

    pub fn send_close(&self) {
        let window = match &self.element {
            LINGMOMappedInternal::Stack(s) => s.active(),
            LINGMOMappedInternal::Window(w) => w.surface(),
            _ => unreachable!(),
        };

        window.close();
    }

    pub fn is_window(&self) -> bool {
        matches!(&self.element, LINGMOMappedInternal::Window(_))
    }

    pub fn is_stack(&self) -> bool {
        matches!(&self.element, LINGMOMappedInternal::Stack(_))
    }

    pub fn stack_ref(&self) -> Option<&LINGMOStack> {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => Some(stack),
            _ => None,
        }
    }

    pub fn convert_to_stack(
        &mut self,
        (output, overlap): (&Output, Rectangle<i32, Logical>),
        theme: LINGMO::Theme,
        appearance: AppearanceConfig,
    ) {
        if let LINGMOMappedInternal::Window(window) = &self.element {
            let surface = window.surface();
            let activated = surface.is_activated(true);
            let handle = window.loop_handle();

            let stack = LINGMOStack::new(std::iter::once(surface), handle, theme, appearance);
            if let Some(geo) = *self.last_geometry.lock().unwrap() {
                stack.set_geometry(geo.to_global(output));
            }
            stack.output_enter(output, overlap);
            stack.set_activate(activated);
            stack.active().send_configure();
            stack.refresh();

            self.element = LINGMOMappedInternal::Stack(stack);
        }
    }

    pub fn convert_to_surface(
        &mut self,
        surface: LINGMOSurface,
        (output, overlap): (&Output, Rectangle<i32, Logical>),
        theme: LINGMO::Theme,
        appearance: AppearanceConfig,
    ) {
        let handle = self.loop_handle();
        surface.try_force_undecorated(false);
        surface.set_tiled(false);
        let window = LINGMOWindow::new(surface, handle, theme, appearance);

        if let Some(geo) = *self.last_geometry.lock().unwrap() {
            window.set_geometry(geo.to_global(output));
        }
        window.output_enter(output, overlap);
        window.set_activate(self.is_activated(true));
        window.surface().send_configure();
        window.refresh();

        self.element = LINGMOMappedInternal::Window(window);
    }

    pub(super) fn loop_handle(&self) -> LoopHandle<'static, crate::state::State> {
        match &self.element {
            LINGMOMappedInternal::Stack(stack) => stack.loop_handle(),
            LINGMOMappedInternal::Window(window) => window.loop_handle(),
            _ => unreachable!(),
        }
    }

    #[cfg(feature = "debug")]
    pub fn set_debug(&self, flag: bool) {
        let mut debug = self.debug.lock().unwrap();
        if flag {
            *debug = Some(smithay_egui::EguiState::new(Rectangle::new(
                (10, 10).into(),
                (100, 100).into(),
            )));
        } else {
            debug.take();
        }
    }

    pub fn push_popup_render_elements<R>(
        &self,
        renderer: &mut R,
        location: smithay::utils::Point<i32, smithay::utils::Physical>,
        scale: smithay::utils::Scale<f64>,
        alpha: f32,
        scanout_node: Option<DrmNode>,
        push: &mut dyn FnMut(LINGMOMappedRenderElement<R>),
    ) where
        R: AsGlowRenderer,
        R::TextureId: Send + Clone + 'static,
        LINGMOMappedRenderElement<R>: RenderElement<R>,
    {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.push_popup_render_elements(
                renderer,
                location,
                scale,
                alpha,
                scanout_node,
                &mut |elem| push(elem.into()),
            ),
            LINGMOMappedInternal::Window(w) => w.push_popup_render_elements(
                renderer,
                location,
                scale,
                alpha,
                scanout_node,
                &mut |elem| push(elem.into()),
            ),
            _ => unreachable!(),
        }
    }

    pub fn shadow_render_element<R, C>(
        &self,
        renderer: &mut R,
        location: smithay::utils::Point<i32, smithay::utils::Physical>,
        max_size: Option<smithay::utils::Size<i32, smithay::utils::Logical>>,
        output_scale: smithay::utils::Scale<f64>,
        scale: f64,
        alpha: f32,
    ) -> Option<C>
    where
        R: AsGlowRenderer,
        R::TextureId: Send + Clone + 'static,
        LINGMOMappedRenderElement<R>: RenderElement<R>,
        C: From<LINGMOMappedRenderElement<R>>,
    {
        if !self.element.alive() {
            return None;
        }

        match &self.element {
            LINGMOMappedInternal::Stack(s) => s
                .shadow_render_element::<R, LINGMOMappedRenderElement<R>>(
                    renderer,
                    location,
                    max_size,
                    output_scale,
                    scale,
                    alpha,
                )
                .map(Into::into),
            LINGMOMappedInternal::Window(w) => w
                .shadow_render_element::<R, LINGMOMappedRenderElement<R>>(
                    renderer,
                    location,
                    max_size,
                    output_scale,
                    scale,
                    alpha,
                )
                .map(Into::into),
            _ => unreachable!(),
        }
    }

    pub fn push_render_elements<R>(
        &self,
        renderer: &mut R,
        location: smithay::utils::Point<i32, smithay::utils::Physical>,
        max_size: Option<smithay::utils::Size<i32, smithay::utils::Logical>>,
        scale: smithay::utils::Scale<f64>,
        alpha: f32,
        scanout_override: Option<bool>,
        scanout_node: Option<DrmNode>,
        push_above: &mut dyn FnMut(LINGMOMappedRenderElement<R>),
        push_below: &mut dyn FnMut(LINGMOMappedRenderElement<R>),
    ) where
        R: AsGlowRenderer,
        R::TextureId: Send + Clone + 'static,
        LINGMOMappedRenderElement<R>: RenderElement<R>,
    {
        #[cfg(feature = "debug")]
        if let Some(debug) = self.debug.lock().unwrap().as_mut() {
            let window = self.active_window();
            let window_geo = window.geometry();
            let (min_size, max_size, size) = (
                window.min_size_without_ssd(),
                window.max_size_without_ssd(),
                window.geometry().size,
            );

            let area = Rectangle::<i32, Logical>::new(
                location.to_f64().to_logical(scale).to_i32_round(),
                self.bbox().size,
            );

            let glow_renderer = renderer.glow_renderer_mut();
            match debug.render(
                |ctx| {
                    egui::Area::new("window".into())
                        .anchor(
                            egui::Align2::RIGHT_TOP,
                            [
                                -window_geo.loc.x as f32 - 10.0,
                                window_geo.loc.y as f32 - 10.0,
                            ],
                        )
                        .show(ctx, |ui| {
                            egui::Frame::NONE
                                .fill(egui::Color32::BLACK)
                                .corner_radius(5.0)
                                .inner_margin(10.0)
                                .show(ui, |ui| {
                                    ui.heading(window.title());
                                    ui.horizontal(|ui| {
                                        ui.label("App ID: ");
                                        ui.label(window.app_id());
                                    });
                                    ui.label(match window.0.underlying_surface() {
                                        WindowSurface::Wayland(_) => "Protocol: Wayland",
                                        WindowSurface::X11(_) => "Protocol: X11",
                                    });
                                    if let WindowSurface::X11(surf) = window.0.underlying_surface()
                                    {
                                        let geo = surf.geometry();
                                        ui.label(format!(
                                            "X11 Geo: {}x{}x{}x{}",
                                            geo.loc.x, geo.loc.y, geo.size.w, geo.size.h
                                        ));
                                    }
                                    ui.horizontal(|ui| {
                                        ui.label("States: ");
                                        if window.is_maximized(true) {
                                            ui.label("🗖");
                                        }
                                        if window.is_fullscreen(true) {
                                            ui.label("⬜");
                                        }
                                        if window.is_activated(true) {
                                            ui.label("🖱");
                                        }
                                        if window.is_resizing(true).is_some() {
                                            ui.label("↔");
                                        }
                                    });

                                    let plot = Plot::new("Sizes")
                                        .legend(Legend::default().position(Corner::RightBottom))
                                        .data_aspect(1.0)
                                        .view_aspect(1.0)
                                        .show_x(false)
                                        .show_y(false)
                                        .width(200.0)
                                        .height(200.0);
                                    plot.show(ui, |plot_ui| {
                                        let center = if let Some(max_size) = max_size {
                                            ((max_size.w + 20) / 2, (max_size.h + 20) / 2)
                                        } else {
                                            (100, 100)
                                        };

                                        if let Some(max_size) = max_size {
                                            let max_size_rect =
                                                Polygon::new(PlotPoints::new(vec![
                                                    [10.0, 10.0],
                                                    [max_size.w as f64 + 10.0, 10.0],
                                                    [
                                                        max_size.w as f64 + 10.0,
                                                        max_size.h as f64 + 10.0,
                                                    ],
                                                    [10.0, max_size.h as f64 + 10.0],
                                                    [10.0, 10.0],
                                                ]));
                                            plot_ui.polygon(
                                                max_size_rect
                                                    .name(format!("{}x{}", max_size.w, max_size.h)),
                                            );
                                        }

                                        let size_rect = Polygon::new(PlotPoints::new(vec![
                                            [
                                                (center.0 - size.w / 2) as f64,
                                                (center.1 - size.h / 2) as f64,
                                            ],
                                            [
                                                (center.0 + size.w / 2) as f64,
                                                (center.1 - size.h / 2) as f64,
                                            ],
                                            [
                                                (center.0 + size.w / 2) as f64,
                                                (center.1 + size.h / 2) as f64,
                                            ],
                                            [
                                                (center.0 - size.w / 2) as f64,
                                                (center.1 + size.h / 2) as f64,
                                            ],
                                            [
                                                (center.0 - size.w / 2) as f64,
                                                (center.1 - size.h / 2) as f64,
                                            ],
                                        ]));
                                        plot_ui.polygon(
                                            size_rect.name(format!("{}x{}", size.w, size.h)),
                                        );

                                        if let Some(min_size) = min_size {
                                            let min_size_rect =
                                                Polygon::new(PlotPoints::new(vec![
                                                    [
                                                        (center.0 - min_size.w / 2) as f64,
                                                        (center.1 - min_size.h / 2) as f64,
                                                    ],
                                                    [
                                                        (center.0 + min_size.w / 2) as f64,
                                                        (center.1 - min_size.h / 2) as f64,
                                                    ],
                                                    [
                                                        (center.0 + min_size.w / 2) as f64,
                                                        (center.1 + min_size.h / 2) as f64,
                                                    ],
                                                    [
                                                        (center.0 - min_size.w / 2) as f64,
                                                        (center.1 + min_size.h / 2) as f64,
                                                    ],
                                                    [
                                                        (center.0 - min_size.w / 2) as f64,
                                                        (center.1 - min_size.h / 2) as f64,
                                                    ],
                                                ]));
                                            plot_ui.polygon(
                                                min_size_rect
                                                    .name(format!("{}x{}", min_size.w, min_size.h)),
                                            );
                                        }
                                    })
                                })
                        });
                },
                glow_renderer,
                area,
                scale.x,
                0.8,
            ) {
                Ok(element) => push_above(element.into()),
                Err(err) => {
                    debug!(?err, "Error rendering debug overlay.");
                }
            }
        };

        match &self.element {
            LINGMOMappedInternal::Stack(s) => s.push_render_elements(
                renderer,
                location,
                max_size,
                scale,
                alpha,
                scanout_override,
                scanout_node,
                &mut |elem| push_above(elem.into()),
                &mut |elem| push_below(elem.into()),
            ),
            LINGMOMappedInternal::Window(w) => w.push_render_elements(
                renderer,
                location,
                max_size,
                scale,
                alpha,
                scanout_override,
                scanout_node,
                &mut |elem| push_above(elem.into()),
                &mut |elem| push_below(elem.into()),
            ),
            _ => unreachable!(),
        }
    }

    pub(crate) fn update_theme(&self, theme: LINGMO::Theme) {
        match &self.element {
            LINGMOMappedInternal::Window(w) => w.set_theme(theme),
            LINGMOMappedInternal::Stack(s) => s.set_theme(theme),
            LINGMOMappedInternal::_GenericCatcher(_) => {}
        }
    }

    pub(crate) fn update_appearance_conf(&self, appearance: &AppearanceConfig) {
        match &self.element {
            LINGMOMappedInternal::Window(w) => w.update_appearance_conf(appearance),
            LINGMOMappedInternal::Stack(s) => s.update_appearance_conf(appearance),
            LINGMOMappedInternal::_GenericCatcher(_) => {}
        }
    }

    pub(crate) fn force_redraw(&self) {
        match &self.element {
            LINGMOMappedInternal::Window(w) => w.force_redraw(),
            LINGMOMappedInternal::Stack(s) => s.force_redraw(),
            LINGMOMappedInternal::_GenericCatcher(_) => {}
        }
    }

    pub fn key(&self) -> LINGMOMappedKey {
        LINGMOMappedKey(match &self.element {
            LINGMOMappedInternal::Stack(stack) => {
                LINGMOMappedKeyInner::Stack(Arc::downgrade(&stack.0.0))
            }
            LINGMOMappedInternal::Window(window) => {
                LINGMOMappedKeyInner::Window(Arc::downgrade(&window.0.0))
            }
            _ => unreachable!(),
        })
    }

    pub fn ssd_height(&self, pending: bool) -> Option<i32> {
        match &self.element {
            LINGMOMappedInternal::Window(w) => (!w.surface().is_decorated(pending))
                .then_some(crate::shell::element::window::SSD_HEIGHT),
            LINGMOMappedInternal::Stack(_) => Some(crate::shell::element::stack::TAB_HEIGHT),
            _ => unreachable!(),
        }
    }

    pub fn corner_radius(&self, geometry_size: Size<i32, Logical>, default_radius: u8) -> [u8; 4] {
        match &self.element {
            LINGMOMappedInternal::Window(w) => w.corner_radius(geometry_size, default_radius),
            LINGMOMappedInternal::Stack(s) => s.corner_radius(geometry_size, default_radius),
            _ => unreachable!(),
        }
    }
}

impl IsAlive for LINGMOMapped {
    fn alive(&self) -> bool {
        self.element.alive()
    }
}

impl SpaceElement for LINGMOMapped {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        SpaceElement::bbox(&self.element)
    }
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        SpaceElement::is_in_input_region(&self.element, point)
    }
    fn set_activate(&self, activated: bool) {
        SpaceElement::set_activate(&self.element, activated)
    }
    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        SpaceElement::output_enter(&self.element, output, overlap)
    }
    fn output_leave(&self, output: &Output) {
        SpaceElement::output_leave(&self.element, output)
    }
    fn geometry(&self) -> Rectangle<i32, Logical> {
        SpaceElement::geometry(&self.element)
    }
    fn z_index(&self) -> u8 {
        SpaceElement::z_index(&self.element)
    }
    #[profiling::function]
    fn refresh(&self) {
        SpaceElement::refresh(&self.element)
    }
}

impl X11Relatable for LINGMOMapped {
    fn is_window(&self, window: &X11Surface) -> bool {
        self.active_window().x11_surface() == Some(window)
    }
}

impl KeyboardTarget<State> for LINGMOMapped {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => KeyboardTarget::enter(s, seat, data, keys, serial),
            LINGMOMappedInternal::Window(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            _ => {}
        }
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => KeyboardTarget::leave(s, seat, data, serial),
            LINGMOMappedInternal::Window(w) => KeyboardTarget::leave(w, seat, data, serial),
            _ => {}
        }
    }
    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => {
                KeyboardTarget::key(s, seat, data, key, state, serial, time)
            }
            LINGMOMappedInternal::Window(w) => {
                KeyboardTarget::key(w, seat, data, key, state, serial, time)
            }
            _ => {}
        }
    }
    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match &self.element {
            LINGMOMappedInternal::Stack(s) => {
                KeyboardTarget::modifiers(s, seat, data, modifiers, serial)
            }
            LINGMOMappedInternal::Window(w) => {
                KeyboardTarget::modifiers(w, seat, data, modifiers, serial)
            }
            _ => {}
        }
    }
}

impl WaylandFocus for LINGMOMapped {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match &self.element {
            LINGMOMappedInternal::Window(w) => {
                w.surface().wl_surface().map(|s| Cow::Owned(s.into_owned()))
            }
            LINGMOMappedInternal::Stack(s) => {
                s.active().wl_surface().map(|s| Cow::Owned(s.into_owned()))
            }
            _ => None,
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match &self.element {
            LINGMOMappedInternal::Window(w) => w.surface().same_client_as(object_id),
            LINGMOMappedInternal::Stack(s) => s.active().same_client_as(object_id),
            _ => false,
        }
    }
}

impl From<LINGMOWindow> for LINGMOMapped {
    fn from(w: LINGMOWindow) -> Self {
        LINGMOMapped {
            element: LINGMOMappedInternal::Window(w),
            maximized_state: Arc::new(Mutex::new(None)),
            tiling_node_id: Arc::new(Mutex::new(None)),
            resize_state: Arc::new(Mutex::new(None)),
            last_geometry: Arc::new(Mutex::new(None)),
            moved_since_mapped: Arc::new(AtomicBool::new(false)),
            floating_tiled: Arc::new(Mutex::new(None)),
            previous_layer: Arc::new(Mutex::new(None)),
            #[cfg(feature = "debug")]
            debug: Arc::new(Mutex::new(None)),
        }
    }
}

impl From<LINGMOStack> for LINGMOMapped {
    fn from(s: LINGMOStack) -> Self {
        LINGMOMapped {
            element: LINGMOMappedInternal::Stack(s),
            maximized_state: Arc::new(Mutex::new(None)),
            tiling_node_id: Arc::new(Mutex::new(None)),
            resize_state: Arc::new(Mutex::new(None)),
            last_geometry: Arc::new(Mutex::new(None)),
            moved_since_mapped: Arc::new(AtomicBool::new(false)),
            floating_tiled: Arc::new(Mutex::new(None)),
            previous_layer: Arc::new(Mutex::new(None)),
            #[cfg(feature = "debug")]
            debug: Arc::new(Mutex::new(None)),
        }
    }
}

pub enum LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: 'static,
{
    Stack(self::stack::LINGMOStackRenderElement<R>),
    Window(self::window::LINGMOWindowRenderElement<R>),
    TiledStack(
        CropRenderElement<
            RelocateRenderElement<RescaleRenderElement<self::stack::LINGMOStackRenderElement<R>>>,
        >,
    ),
    TiledWindow(
        CropRenderElement<
            RelocateRenderElement<RescaleRenderElement<self::window::LINGMOWindowRenderElement<R>>>,
        >,
    ),
    TiledOverlay(
        CropRenderElement<RelocateRenderElement<RescaleRenderElement<PixelShaderElement>>>,
    ),
    MovingStack(
        RelocateRenderElement<RescaleRenderElement<self::stack::LINGMOStackRenderElement<R>>>,
    ),
    MovingWindow(
        RelocateRenderElement<RescaleRenderElement<self::window::LINGMOWindowRenderElement<R>>>,
    ),
    GrabbedStack(RescaleRenderElement<self::stack::LINGMOStackRenderElement<R>>),
    GrabbedWindow(RescaleRenderElement<self::window::LINGMOWindowRenderElement<R>>),
    FocusIndicator(PixelShaderElement),
    Overlay(PixelShaderElement),
    StackHoverIndicator(IcedRenderElement<R>),
    #[cfg(feature = "debug")]
    Egui(TextureRenderElement<GlesTexture>),
}

impl<R> Element for LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: Send + 'static,
{
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.id(),
            LINGMOMappedRenderElement::Window(elem) => elem.id(),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.id(),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.id(),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.id(),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.id(),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.id(),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.id(),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.id(),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.id(),
            LINGMOMappedRenderElement::Overlay(elem) => elem.id(),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.id(),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.id(),
        }
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::Window(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::Overlay(elem) => elem.current_commit(),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.current_commit(),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.current_commit(),
        }
    }

    fn src(&self) -> Rectangle<f64, smithay::utils::Buffer> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.src(),
            LINGMOMappedRenderElement::Window(elem) => elem.src(),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.src(),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.src(),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.src(),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.src(),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.src(),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.src(),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.src(),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.src(),
            LINGMOMappedRenderElement::Overlay(elem) => elem.src(),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.src(),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.src(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::Window(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::Overlay(elem) => elem.geometry(scale),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.geometry(scale),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.geometry(scale),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.location(scale),
            LINGMOMappedRenderElement::Window(elem) => elem.location(scale),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.location(scale),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.location(scale),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.location(scale),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.location(scale),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.location(scale),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.location(scale),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.location(scale),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.location(scale),
            LINGMOMappedRenderElement::Overlay(elem) => elem.location(scale),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.location(scale),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.location(scale),
        }
    }

    fn transform(&self) -> smithay::utils::Transform {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.transform(),
            LINGMOMappedRenderElement::Window(elem) => elem.transform(),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.transform(),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.transform(),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.transform(),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.transform(),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.transform(),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.transform(),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.transform(),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.transform(),
            LINGMOMappedRenderElement::Overlay(elem) => elem.transform(),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.transform(),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.transform(),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::Window(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::Overlay(elem) => elem.damage_since(scale, commit),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => {
                elem.damage_since(scale, commit)
            }
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::Window(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::Overlay(elem) => elem.opaque_regions(scale),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.opaque_regions(scale),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.alpha(),
            LINGMOMappedRenderElement::Window(elem) => elem.alpha(),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.alpha(),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.alpha(),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.alpha(),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.alpha(),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.alpha(),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.alpha(),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.alpha(),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.alpha(),
            LINGMOMappedRenderElement::Overlay(elem) => elem.alpha(),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.alpha(),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.alpha(),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.kind(),
            LINGMOMappedRenderElement::Window(elem) => elem.kind(),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.kind(),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.kind(),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.kind(),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.kind(),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.kind(),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.kind(),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.kind(),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.kind(),
            LINGMOMappedRenderElement::Overlay(elem) => elem.kind(),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.kind(),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.kind(),
        }
    }

    fn is_framebuffer_effect(&self) -> bool {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::Window(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::TiledOverlay(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::MovingStack(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::FocusIndicator(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::Overlay(elem) => elem.is_framebuffer_effect(),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => elem.is_framebuffer_effect(),
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => elem.is_framebuffer_effect(),
        }
    }
}

impl<R> RenderElement<R> for LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: Send + 'static,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::Window(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::TiledStack(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::TiledWindow(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::TiledOverlay(elem) => RenderElement::<GlowRenderer>::draw(
                elem,
                R::glow_frame_mut(frame),
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            )
            .map_err(R::from_gles_error),
            LINGMOMappedRenderElement::MovingStack(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::MovingWindow(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::GrabbedStack(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::GrabbedWindow(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            LINGMOMappedRenderElement::FocusIndicator(elem) => RenderElement::<GlowRenderer>::draw(
                elem,
                R::glow_frame_mut(frame),
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            )
            .map_err(R::from_gles_error),
            LINGMOMappedRenderElement::Overlay(elem) => RenderElement::<GlowRenderer>::draw(
                elem,
                R::glow_frame_mut(frame),
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            )
            .map_err(R::from_gles_error),
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions, cache)
            }
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => {
                let glow_frame = R::glow_frame_mut(frame);
                RenderElement::<GlowRenderer>::draw(
                    elem,
                    glow_frame,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    cache,
                )
                .map_err(R::from_gles_error)
            }
        }
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::Window(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::TiledStack(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::TiledWindow(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::TiledOverlay(elem) => {
                elem.underlying_storage(renderer.glow_renderer_mut())
            }
            LINGMOMappedRenderElement::MovingStack(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::MovingWindow(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::GrabbedStack(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::GrabbedWindow(elem) => elem.underlying_storage(renderer),
            LINGMOMappedRenderElement::FocusIndicator(elem) => {
                elem.underlying_storage(renderer.glow_renderer_mut())
            }
            LINGMOMappedRenderElement::Overlay(elem) => {
                elem.underlying_storage(renderer.glow_renderer_mut())
            }
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => {
                elem.underlying_storage(renderer)
            }
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => {
                let glow_renderer = renderer.glow_renderer_mut();
                elem.underlying_storage(glow_renderer)
            }
        }
    }

    fn capture_framebuffer(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), R::Error> {
        match self {
            LINGMOMappedRenderElement::Stack(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::Window(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::TiledStack(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::TiledWindow(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::TiledOverlay(elem) => {
                RenderElement::<GlowRenderer>::capture_framebuffer(
                    elem,
                    R::glow_frame_mut(frame),
                    src,
                    dst,
                    cache,
                )
                .map_err(R::from_gles_error)
            }
            LINGMOMappedRenderElement::MovingStack(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::MovingWindow(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::GrabbedStack(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::GrabbedWindow(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            LINGMOMappedRenderElement::FocusIndicator(elem) => {
                RenderElement::<GlowRenderer>::capture_framebuffer(
                    elem,
                    R::glow_frame_mut(frame),
                    src,
                    dst,
                    cache,
                )
                .map_err(R::from_gles_error)
            }
            LINGMOMappedRenderElement::Overlay(elem) => {
                RenderElement::<GlowRenderer>::capture_framebuffer(
                    elem,
                    R::glow_frame_mut(frame),
                    src,
                    dst,
                    cache,
                )
                .map_err(R::from_gles_error)
            }
            LINGMOMappedRenderElement::StackHoverIndicator(elem) => {
                elem.capture_framebuffer(frame, src, dst, cache)
            }
            #[cfg(feature = "debug")]
            LINGMOMappedRenderElement::Egui(elem) => {
                let glow_frame = R::glow_frame_mut(frame);
                RenderElement::<GlowRenderer>::capture_framebuffer(
                    elem, glow_frame, src, dst, cache,
                )
                .map_err(R::from_gles_error)
            }
        }
    }
}

impl<R> From<stack::LINGMOStackRenderElement<R>> for LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: 'static,
    LINGMOMappedRenderElement<R>: RenderElement<R>,
{
    fn from(elem: stack::LINGMOStackRenderElement<R>) -> Self {
        LINGMOMappedRenderElement::Stack(elem)
    }
}
impl<R> From<window::LINGMOWindowRenderElement<R>> for LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: 'static,
    LINGMOMappedRenderElement<R>: RenderElement<R>,
{
    fn from(elem: window::LINGMOWindowRenderElement<R>) -> Self {
        LINGMOMappedRenderElement::Window(elem)
    }
}

impl<R> From<PixelShaderElement> for LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: 'static,
    LINGMOMappedRenderElement<R>: RenderElement<R>,
{
    fn from(elem: PixelShaderElement) -> Self {
        LINGMOMappedRenderElement::FocusIndicator(elem)
    }
}

impl<R> From<IcedRenderElement<R>> for LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: 'static,
    LINGMOMappedRenderElement<R>: RenderElement<R>,
{
    fn from(elem: IcedRenderElement<R>) -> Self {
        LINGMOMappedRenderElement::StackHoverIndicator(elem)
    }
}

#[cfg(feature = "debug")]
impl<R> From<TextureRenderElement<GlesTexture>> for LINGMOMappedRenderElement<R>
where
    R: AsGlowRenderer,
    R::TextureId: 'static,
    LINGMOMappedRenderElement<R>: RenderElement<R>,
{
    fn from(elem: TextureRenderElement<GlesTexture>) -> Self {
        LINGMOMappedRenderElement::Egui(elem)
    }
}
