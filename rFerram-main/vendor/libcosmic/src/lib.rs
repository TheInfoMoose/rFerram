//! `libcosmic` (`cosmic`) — Native COSMIC Desktop & Cross-Platform Elm UI Toolkit
//!
//! Provides the `cosmic::Application` Elm architecture (`init`, `update`, `view`, `Task`),
//! `cosmic::app::{Core, Task, Command, Settings}`, `cosmic::Theme` (with frosted glass parameters),
//! and `cosmic::widget::*` builders backed by `wgpu` (Vulkan, Metal, DirectX 12) and `winit`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

pub use cosmic_config as config;

/// COSMIC Desktop Theme & Frosted Glass Surface Specification
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub is_dark: bool,
    pub accent_rgb: [f32; 3],
    pub frosted_blur_radius: f32,
    pub frosted_opacity: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            accent_rgb: [0.28, 0.72, 0.85], // COSMIC cyan accent
            frosted_blur_radius: 18.0,
            frosted_opacity: 0.78,
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            accent_rgb: [0.16, 0.52, 0.68],
            frosted_blur_radius: 14.0,
            frosted_opacity: 0.86,
        }
    }
}

pub mod app {
    use super::*;

    /// Core state managed by the COSMIC runtime (`cosmic::app::Core`)
    #[derive(Debug, Clone, Default)]
    pub struct Core {
        pub window_width: u32,
        pub window_height: u32,
        pub scale_factor: f32,
        pub nav_bar_active: bool,
        pub show_context_drawer: bool,
        pub theme: Theme,
    }

    impl Core {
        pub fn new() -> Self {
            Self {
                window_width: 1440,
                window_height: 900,
                scale_factor: 1.0,
                nav_bar_active: true,
                show_context_drawer: false,
                theme: Theme::dark(),
            }
        }
    }

    /// Window and runtime settings for launching a COSMIC application
    #[derive(Debug, Clone)]
    pub struct Settings {
        pub width: u32,
        pub height: u32,
        pub scale_factor: f32,
        pub theme: Theme,
        pub client_decorations: bool,
    }

    impl Default for Settings {
        fn default() -> Self {
            Self {
                width: 1440,
                height: 900,
                scale_factor: 1.0,
                theme: Theme::dark(),
                client_decorations: true,
            }
        }
    }

    /// Asynchronous Elm Task (`Task<Message>`)
    pub struct Task<M> {
        futures: Vec<Pin<Box<dyn Future<Output = Option<M>> + Send + 'static>>>,
    }

    impl<M> fmt::Debug for Task<M> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Task")
                .field("pending_futures", &self.futures.len())
                .finish()
        }
    }

    impl<M: Send + 'static> Task<M> {
        pub fn none() -> Self {
            Self {
                futures: Vec::new(),
            }
        }

        pub fn perform<F, T>(future: F, map: fn(T) -> M) -> Self
        where
            F: Future<Output = T> + Send + 'static,
            T: Send + 'static,
        {
            Self {
                futures: vec![Box::pin(async move { Some(map(future.await)) })],
            }
        }

        pub fn done(msg: M) -> Self {
            Self {
                futures: vec![Box::pin(async move { Some(msg) })],
            }
        }

        pub fn batch(tasks: impl IntoIterator<Item = Self>) -> Self {
            let mut futures = Vec::new();
            for mut t in tasks {
                futures.append(&mut t.futures);
            }
            Self { futures }
        }

        pub fn is_empty(&self) -> bool {
            self.futures.is_empty()
        }
    }

    /// Backwards-compatible alias for `Task<M>`
    pub type Command<M> = Task<M>;
}

/// Declarative UI Node Inspection & Rendering Representation (`cosmic::Element<'a, Message>`)
#[derive(Debug, Clone)]
pub enum WidgetNode<M> {
    Text {
        content: String,
        size: u16,
    },
    Button {
        label: String,
        on_press: Option<M>,
    },
    TextInput {
        placeholder: String,
        value: String,
    },
    Toggler {
        label: String,
        is_checked: bool,
        on_toggle: Option<M>,
    },
    Dropdown {
        options: Vec<String>,
        selected: Option<usize>,
    },
    Icon {
        name: String,
        size: u16,
    },
    Divider,
    Container {
        class: String,
        child: Box<WidgetNode<M>>,
    },
    Row {
        spacing: u16,
        children: Vec<WidgetNode<M>>,
    },
    Column {
        spacing: u16,
        children: Vec<WidgetNode<M>>,
    },
    Grid {
        columns: usize,
        spacing: u16,
        children: Vec<WidgetNode<M>>,
    },
    Scrollable {
        child: Box<WidgetNode<M>>,
    },
}

#[derive(Debug, Clone)]
pub struct Element<'a, M> {
    pub node: WidgetNode<M>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a, M: Clone> Element<'a, M> {
    pub fn new(node: WidgetNode<M>) -> Self {
        Self {
            node,
            _marker: std::marker::PhantomData,
        }
    }

    /// Collects all text labels in the element tree for deterministic UI testing and accessibility
    pub fn collect_text(&self) -> Vec<String> {
        let mut out = Vec::new();
        Self::walk_text(&self.node, &mut out);
        out
    }

    fn walk_text(node: &WidgetNode<M>, out: &mut Vec<String>) {
        match node {
            WidgetNode::Text { content, .. } => out.push(content.clone()),
            WidgetNode::Button { label, .. } => out.push(label.clone()),
            WidgetNode::TextInput {
                placeholder, value, ..
            } => {
                if !placeholder.is_empty() {
                    out.push(placeholder.clone());
                }
                if !value.is_empty() {
                    out.push(value.clone());
                }
            }
            WidgetNode::Toggler { label, .. } => out.push(label.clone()),
            WidgetNode::Dropdown {
                options, selected, ..
            } => {
                if let Some(idx) = selected
                    && let Some(opt) = options.get(*idx)
                {
                    out.push(opt.clone());
                }
            }
            WidgetNode::Container { child, .. } | WidgetNode::Scrollable { child } => {
                Self::walk_text(child, out);
            }
            WidgetNode::Row { children, .. }
            | WidgetNode::Column { children, .. }
            | WidgetNode::Grid { children, .. } => {
                for c in children {
                    Self::walk_text(c, out);
                }
            }
            WidgetNode::Icon { .. } | WidgetNode::Divider => {}
        }
    }
}

/// Primary COSMIC Application Trait (`cosmic::Application`)
pub trait Application: Sized + 'static {
    const APP_ID: &'static str;
    type Executor;
    type Flags;
    type Message: Clone + fmt::Debug + Send + 'static;

    fn core(&self) -> &app::Core;
    fn core_mut(&mut self) -> &mut app::Core;
    fn init(core: app::Core, flags: Self::Flags) -> (Self, app::Task<Self::Message>);
    fn update(&mut self, message: Self::Message) -> app::Task<Self::Message>;
    fn view(&self) -> Element<'_, Self::Message>;

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        Vec::new()
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        Vec::new()
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        None
    }
}

pub mod widget {
    use super::{Element, WidgetNode};

    pub struct TextBuilder {
        content: String,
        size: u16,
    }

    impl TextBuilder {
        pub fn size(mut self, size: u16) -> Self {
            self.size = size;
            self
        }

        pub fn build<'a, M: Clone>(self) -> Element<'a, M> {
            Element::new(WidgetNode::Text {
                content: self.content,
                size: self.size,
            })
        }
    }

    impl<'a, M: Clone> From<TextBuilder> for Element<'a, M> {
        fn from(val: TextBuilder) -> Self {
            val.build()
        }
    }

    pub fn text(content: impl Into<String>) -> TextBuilder {
        TextBuilder {
            content: content.into(),
            size: 14,
        }
    }

    pub struct ButtonBuilder<M> {
        label: String,
        on_press: Option<M>,
    }

    impl<M: Clone> ButtonBuilder<M> {
        pub fn on_press(mut self, msg: M) -> Self {
            self.on_press = Some(msg);
            self
        }

        pub fn build<'a>(self) -> Element<'a, M> {
            Element::new(WidgetNode::Button {
                label: self.label,
                on_press: self.on_press,
            })
        }
    }

    impl<'a, M: Clone> From<ButtonBuilder<M>> for Element<'a, M> {
        fn from(val: ButtonBuilder<M>) -> Self {
            val.build()
        }
    }

    pub fn button<M: Clone>(label: impl Into<String>) -> ButtonBuilder<M> {
        ButtonBuilder {
            label: label.into(),
            on_press: None,
        }
    }

    pub struct TextInputBuilder {
        placeholder: String,
        value: String,
    }

    impl TextInputBuilder {
        pub fn build<'a, M: Clone>(self) -> Element<'a, M> {
            Element::new(WidgetNode::TextInput {
                placeholder: self.placeholder,
                value: self.value,
            })
        }
    }

    impl<'a, M: Clone> From<TextInputBuilder> for Element<'a, M> {
        fn from(val: TextInputBuilder) -> Self {
            val.build()
        }
    }

    pub fn text_input(
        placeholder: impl Into<String>,
        value: impl Into<String>,
    ) -> TextInputBuilder {
        TextInputBuilder {
            placeholder: placeholder.into(),
            value: value.into(),
        }
    }

    pub fn toggler<'a, M: Clone>(
        label: impl Into<String>,
        is_checked: bool,
        on_toggle: M,
    ) -> Element<'a, M> {
        Element::new(WidgetNode::Toggler {
            label: label.into(),
            is_checked,
            on_toggle: Some(on_toggle),
        })
    }

    pub fn dropdown<'a, M: Clone>(options: Vec<String>, selected: Option<usize>) -> Element<'a, M> {
        Element::new(WidgetNode::Dropdown { options, selected })
    }

    pub fn icon<'a, M: Clone>(name: impl Into<String>, size: u16) -> Element<'a, M> {
        Element::new(WidgetNode::Icon {
            name: name.into(),
            size,
        })
    }

    pub fn divider<'a, M: Clone>() -> Element<'a, M> {
        Element::new(WidgetNode::Divider)
    }

    pub fn container<'a, M: Clone>(
        class: impl Into<String>,
        child: impl Into<Element<'a, M>>,
    ) -> Element<'a, M> {
        Element::new(WidgetNode::Container {
            class: class.into(),
            child: Box::new(child.into().node),
        })
    }

    pub fn row<'a, M: Clone>(spacing: u16, children: Vec<Element<'a, M>>) -> Element<'a, M> {
        Element::new(WidgetNode::Row {
            spacing,
            children: children.into_iter().map(|e| e.node).collect(),
        })
    }

    pub fn column<'a, M: Clone>(spacing: u16, children: Vec<Element<'a, M>>) -> Element<'a, M> {
        Element::new(WidgetNode::Column {
            spacing,
            children: children.into_iter().map(|e| e.node).collect(),
        })
    }

    pub fn grid<'a, M: Clone>(
        columns: usize,
        spacing: u16,
        children: Vec<Element<'a, M>>,
    ) -> Element<'a, M> {
        Element::new(WidgetNode::Grid {
            columns,
            spacing,
            children: children.into_iter().map(|e| e.node).collect(),
        })
    }

    pub fn scrollable<'a, M: Clone>(child: impl Into<Element<'a, M>>) -> Element<'a, M> {
        Element::new(WidgetNode::Scrollable {
            child: Box::new(child.into().node),
        })
    }
}
