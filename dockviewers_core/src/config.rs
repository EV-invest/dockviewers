//! Host-supplied configuration for the packed layout. Keybinds plus host-registered chords;
//! passed once into [`PackedState`]. Binds match the **produced
//! character** (`KeyboardEvent.key()`), so they follow the user's keyboard layout instead of a
//! hardcoded QWERTY physical position.

use std::rc::Rc;

use crate::state::PackedState;

/// A single chord: the character the key produces, plus the non-shift modifiers held. Shift is
/// already baked into `key` (`"u"` vs `"U"`), so it isn't a separate flag.
#[derive(Clone, Copy, PartialEq)]
pub struct Keybind {
	/// Matched verbatim against `KeyboardEvent.key()` — e.g. `"u"`, `"U"`, `"Delete"`, `"f"`.
	pub key: &'static str,
	pub alt: bool,
	pub ctrl: bool,
}

impl Keybind {
	pub(crate) fn matches(&self, key: &str, alt: bool, ctrl: bool) -> bool {
		self.key == key && self.alt == alt && self.ctrl == ctrl
	}
}

/// Chords acting on the layout / the focused pane. Defaults: `u` / `U` for the undo tree,
/// `Backspace` to close the focused pane, `f` to toggle maximize on it, `?` for the keybind hint.
/// They never fire while an editable field is focused (see the binding's listener), so bare letters
/// don't hijack typing.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Keybinds {
	pub undo: Keybind = Keybind { key: "u", alt: false, ctrl: false },
	pub redo: Keybind = Keybind { key: "U", alt: false, ctrl: false },
	pub close: Keybind = Keybind { key: "Backspace", alt: false, ctrl: false },
	pub maximize: Keybind = Keybind { key: "f", alt: false, ctrl: false },
	pub help: Keybind = Keybind { key: "?", alt: false, ctrl: false },
	/// Cache this band's layout in `localStorage`, so this browser starts from it.
	pub save: Keybind = Keybind { key: "s", alt: true, ctrl: false },
	/// Hand this band's layout to the host, to become the default *other* clients start from.
	pub publish: Keybind = Keybind { key: "S", alt: true, ctrl: false },
}

/// A host-registered chord's action: arbitrary code over the live layout. Framework-agnostic —
/// the binding invokes it against its reactive cell's [`PackedState`]. `Rc` (not `Box`) so
/// [`Config`] stays `Clone` for a binding that hands it around by prop.
pub type Action = Rc<dyn Fn(&mut PackedState)>;

/// What a save chord produced, handed to [`Config::on_save`].
pub enum Saved {
	/// [`Keybinds::save`] — already written to `localStorage` under `band`'s key. Feedback only.
	Cached { band: Band },
	/// [`Keybinds::publish`] — the framework does nothing with this; the host persists `json`
	/// wherever it wants, to become the default fresh clients start from.
	Published { band: Band, json: String },
}

#[derive(Clone, Default)]
pub struct Config {
	pub keybinds: Keybinds,
	/// Host-registered chords, each running arbitrary code over the live layout. Built-ins win on
	/// collision (the listener tries them first); the action mutates the same [`PackedState`] the
	/// host script drives, so it can `save()` the current layout. A bare `Vec` is the whole API.
	pub actions: Vec<(Keybind, Action)>,
	/// Desktop (`Xl`) column count: how many grid steps span the container's width on a wide screen.
	/// Smaller [`Breakpoint`]s scale this down so the *physical* step stays ~constant and tiles reflow
	/// instead of shrinking (see `Breakpoint::scale_cols`). The rendered horizontal step is
	/// `container_width / cols`, so within a band the layout still stretches to fill. A finer grid
	/// (more steps) gives smaller resize/placement increments.
	pub steps: u32 = 64,
	/// Row count — the vertical twin of [`steps`](Self::steps), but *not* scaled per [`Breakpoint`]:
	/// a narrow band is usually a taller device, so the `container_height / rows` vertical step
	/// already tracks the screen without help. Dividing by a *fixed* row count, not the used rows,
	/// keeps the whitespace-below look. The default ≈ a square step on a 16∶9 container (`64 × 9/16`).
	pub rows: u32 = 36,
	/// `localStorage` namespace for the built-in seed cache; the [`Band`] is appended. `None` ⇒ no
	/// client cache at all, and every band entry reports a miss for the host to seed.
	pub storage_key: Option<String> = None,
	/// Fires after a save chord. `None` ⇒ [`publish`](Keybinds::publish) is unbound, since the
	/// framework has nowhere to put a published layout on its own.
	pub on_save: Option<Rc<dyn Fn(Saved)>> = None,
}

/// Config never changes at runtime; a binding compares it only to decide whether to re-seed. The
/// action closures have no identity, so equality is over the keybinds and the registered chords.
impl PartialEq for Config {
	fn eq(&self, other: &Self) -> bool {
		self.keybinds == other.keybinds
			&& self.steps == other.steps
			&& self.rows == other.rows
			&& self.storage_key == other.storage_key
			&& self.on_save.is_some() == other.on_save.is_some()
			&& self.actions.len() == other.actions.len()
			&& self.actions.iter().zip(&other.actions).all(|(a, b)| a.0 == b.0)
	}
}

/// Responsive width bands — Bootstrap's xs/sm/md/lg/xl boundaries (CSS px). The grid's column and
/// row counts are derived per band so the *physical* step size stays ~constant across devices: a
/// phone gets fewer steps than a desktop, so the same tiles reflow and stack down instead of
/// shrinking to illegibility. The count is fixed within a band (the grid still stretches to fill),
/// so a layout has one stable signature per band. Layouts are stored per [`Band`], not per
/// breakpoint: neighbouring bands differ only in column count, and a phone-sized arrangement is
/// worth keeping apart from a desktop one, a 16-column difference is not.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, Eq, Hash, PartialEq, serde::Serialize)]
pub enum Breakpoint {
	Xs,
	Sm,
	Md,
	Lg,
	#[default]
	Xl,
}

impl Breakpoint {
	/// Classify a container width (CSS px) into its band (Bootstrap's boundaries).
	pub(crate) fn of(width: f64) -> Self {
		match width {
			w if w < 576.0 => Self::Xs,
			w if w < 768.0 => Self::Sm,
			w if w < 992.0 => Self::Md,
			w if w < 1200.0 => Self::Lg,
			_ => Self::Xl,
		}
	}

	/// Design width the band scales against — its upper edge (the next band's [`of`](Self::of)
	/// threshold), with `Xl` (open-ended) capped at 1600.
	const fn design(self) -> f64 {
		match self {
			Self::Xs => 576.0,
			Self::Sm => 768.0,
			Self::Md => 992.0,
			Self::Lg => 1200.0,
			Self::Xl => 1600.0,
		}
	}

	/// Scale a desktop-tuned ([`Config`]) column count down to this band (≥ 1). `base · design /
	/// design(Xl)` holds the horizontal step's physical px ~constant — scaling the count by the
	/// band's width gives the same step size, hence the reflow rather than a shrink. Only *columns*
	/// scale: rows stay put because a narrow band is usually a taller device, so its height (and the
	/// `height / rows` vertical step) already tracks the screen on its own.
	pub(crate) fn scale_cols(self, base: u32) -> u32 {
		((base as f64 * self.design() / Self::Xl.design()).round() as u32).max(1)
	}

	pub(crate) const fn band(self) -> Band {
		match self {
			Self::Xs | Self::Sm => Band::Sm,
			Self::Md => Band::Md,
			Self::Lg | Self::Xl => Band::Xl,
		}
	}
}

/// The unit a layout is stored under: phone, tablet, desktop. Coarser than [`Breakpoint`], which
/// exists to keep the *step* physically constant and would otherwise force a separate saved layout
/// for a 200px width difference nobody rearranges tiles over.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, Eq, Hash, PartialEq, serde::Serialize)]
pub enum Band {
	Sm,
	Md,
	#[default]
	Xl,
}

impl std::fmt::Display for Band {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::Sm => "sm",
			Self::Md => "md",
			Self::Xl => "xl",
		})
	}
}

impl std::fmt::Display for Breakpoint {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::Xs => "xs",
			Self::Sm => "sm",
			Self::Md => "md",
			Self::Lg => "lg",
			Self::Xl => "xl",
		})
	}
}
