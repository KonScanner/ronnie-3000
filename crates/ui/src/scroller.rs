//! Common wheel, keyboard, focus-reveal, and click-versus-pan scrolling.

/// Scroll direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Horizontal lanes and card rows.
    Horizontal,
    /// Vertical pages and lists.
    Vertical,
}

/// Browser-like scroll keys along one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKey {
    /// Arrow toward the start.
    Back,
    /// Arrow toward the end.
    Forward,
    /// Page toward the start.
    PageUp,
    /// Page toward the end.
    PageDown,
    /// Start of content.
    Home,
    /// End of content.
    End,
}

/// Whether a pointer release is a click or a completed pan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickOutcome {
    /// The press stayed below the pan threshold.
    Click,
    /// The press moved at least eight pixels and must not click.
    Pan,
}

#[derive(Debug, Clone, Copy)]
struct Pan {
    pointer: i32,
    position: i32,
    active: bool,
}

/// Shared one-axis scroll state.
#[derive(Debug)]
pub struct Scroller {
    axis: Axis,
    position: i32,
    visible: i32,
    content: i32,
    wheel_remainder: f32,
    pan: Option<Pan>,
}

impl Scroller {
    /// Creates a scroller with a viewport and content length in design pixels.
    #[must_use]
    pub fn new(axis: Axis, visible: i32, content: i32) -> Self {
        Self {
            axis,
            position: 0,
            visible: visible.max(0),
            content: content.max(0),
            wheel_remainder: 0.0,
            pan: None,
        }
    }

    /// Current clamped view position.
    #[must_use]
    pub fn position(&self) -> i32 {
        self.position
    }

    /// Whether a drag has crossed the pan threshold.
    #[must_use]
    pub fn panning(&self) -> bool {
        self.pan.is_some_and(|pan| pan.active)
    }

    fn maximum(&self) -> i32 {
        (self.content - self.visible).max(0)
    }

    fn set_position(&mut self, position: i32) {
        self.position = position.clamp(0, self.maximum());
    }

    /// Applies arrow, page, home, or end scrolling. Returns false when content fits.
    pub fn scroll_by_key(&mut self, key: ScrollKey) -> bool {
        if self.content <= self.visible {
            return false;
        }
        let page = 40.max(self.visible - 40);
        let target = match key {
            ScrollKey::Back => self.position - 40,
            ScrollKey::Forward => self.position + 40,
            ScrollKey::PageUp => self.position - page,
            ScrollKey::PageDown => self.position + page,
            ScrollKey::Home => 0,
            ScrollKey::End => self.maximum(),
        };
        self.set_position(target);
        true
    }

    /// Applies wheel or trackpad motion with dominant-axis selection and fractional carry.
    /// Returns false for modified gestures, subpixel-only input, or an already clamped edge.
    pub fn wheel(&mut self, delta_x: f32, delta_y: f32, modified: bool) -> bool {
        if modified || self.content <= self.visible {
            return false;
        }
        let delta = match self.axis {
            Axis::Horizontal if delta_y.abs() > delta_x.abs() => delta_y,
            Axis::Horizontal => delta_x,
            Axis::Vertical => delta_y,
        };
        self.wheel_remainder += delta * 224.0;
        let step = self.wheel_remainder.trunc() as i32;
        self.wheel_remainder -= step as f32;
        if step == 0 {
            return false;
        }
        let before = self.position;
        self.set_position(before - step);
        self.position != before
    }

    /// Reveals the least amount needed to show a target with margin.
    pub fn reveal(&mut self, start: i32, end: i32, margin: i32) {
        let start = start - margin;
        let end = end + margin;
        if start < self.position || end - start > self.visible {
            self.set_position(start);
        } else if end > self.position + self.visible {
            self.set_position(end - self.visible);
        }
    }

    /// Starts a possible click or drag pan.
    pub fn press(&mut self, pointer: i32) {
        self.pan = Some(Pan {
            pointer,
            position: self.position,
            active: false,
        });
    }

    /// Moves a pressed pointer. Crossing eight pixels spends the click and starts a pan.
    pub fn drag_to(&mut self, pointer: i32) -> ClickOutcome {
        let Some(mut pan) = self.pan else {
            return ClickOutcome::Click;
        };
        let delta = pointer - pan.pointer;
        if !pan.active && delta.abs() <= 8 {
            self.pan = Some(pan);
            return ClickOutcome::Click;
        }
        pan.active = true;
        self.pan = Some(pan);
        self.set_position(pan.position - delta);
        ClickOutcome::Pan
    }

    /// Finishes a press and reports whether its click must be suppressed.
    pub fn release(&mut self) -> ClickOutcome {
        match self.pan.take() {
            Some(pan) if pan.active => ClickOutcome::Pan,
            _ => ClickOutcome::Click,
        }
    }
}
