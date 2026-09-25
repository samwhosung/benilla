//! The glue screens' art, loaded off the player's own patch chain, plus the texcoord tables of
//! `CharacterCreate.lua` and the glue palette. Every piece is optional: with no client data the
//! screens fall back to plain text buttons.

use bevy::prelude::*;

use benilla_assets::WorldAssets;

use super::add_material::AddUiMaterial;
use super::backdrop::BackdropEdges;

// ── The authored glue palette ────────────────────────────────────────────────────────────────────

/// `GlueFontNormal*`'s gold (GlueFonts.xml: 1.0, 0.78, 0).
pub(crate) const GOLD: Color = Color::srgb(1.0, 0.78, 0.0);
/// `GlueFontDisable*`'s gray.
pub(crate) const DIM: Color = Color::srgb(0.5, 0.5, 0.5);
/// The info bodies' `GlueFontCharacterCreate`: white, from `GlueFontHighlightSmall`.
pub(crate) const INFO_TEXT: Color = Color::WHITE;
/// The glue screens' root fill, ours (Alliance cool, Horde warm on the create screen), seen only
/// where no 3D scene draws over it.
pub(crate) const BACKDROP: Color = Color::srgb(0.05, 0.06, 0.08);
pub(crate) const BACKDROP_ALLIANCE: Color = Color::srgb(0.05, 0.06, 0.10);
pub(crate) const BACKDROP_HORDE: Color = Color::srgb(0.09, 0.05, 0.05);
/// `FACTION_BACKDROP_COLOR_TABLE` (CharacterCreate.lua): the info panels take only the fill (the
/// border tint is commented out); the name box takes both, always the Alliance row.
pub(crate) const ALLIANCE_BORDER: Color = Color::srgb(0.5, 0.5, 0.5);
pub(crate) const ALLIANCE_FILL: Color = Color::srgb(0.09, 0.09, 0.19);
pub(crate) const HORDE_FILL: Color = Color::srgb(0.19, 0.05, 0.05);
/// Fallbacks with no client art; `FALLBACK_ALPHA` matches the ~0.8 alpha baked into
/// `UI-Tooltip-Background`.
pub(crate) const BTN_BG: Color = Color::srgba(1.0, 1.0, 1.0, 0.05);
pub(crate) const BTN_HOVER: Color = Color::srgba(1.0, 1.0, 1.0, 0.14);
pub(crate) const FALLBACK_ALPHA: f32 = 0.8;

/// The info panels' backdrop: `TextPanel-Border`, edge/tile 32 (CharacterCreate.xml).
pub(crate) const PANEL_EDGE: f32 = 32.0;
/// The name box's backdrop: `Glue-Tooltip-Border`, edge/tile 16.
pub(crate) const NAME_EDGE: f32 = 16.0;

// ── The art set ──────────────────────────────────────────────────────────────────────────────────

/// One spinner arrow's states, `Glue-{Left,Right}Arrow-Button-{Up,Down,Highlight}`.
pub(crate) struct ArrowArt {
    pub(crate) up: Handle<Image>,
    pub(crate) down: Option<Handle<Image>>,
    pub(crate) hi: Option<Handle<AddUiMaterial>>,
}

/// One scrollbar arrow's states (`UI-ScrollBar-Scroll{Up,Down}Button-*`), the 16² button in the
/// center quarter of a 32² sheet ([`SCROLL_BTN_TC`]).
pub(crate) struct ScrollBtnArt {
    pub(crate) up: Handle<Image>,
    pub(crate) down: Handle<Image>,
    pub(crate) dis: Handle<Image>,
    pub(crate) hi: Handle<AddUiMaterial>,
    pub(crate) size: Vec2,
}

/// The login Save Account Name checkbox's states (`UI-CheckBox-*`, AccountLogin.xml).
pub(crate) struct CheckboxArt {
    pub(crate) up: Handle<Image>,
    pub(crate) down: Handle<Image>,
    pub(crate) checked: Handle<Image>,
    pub(crate) hi: Option<Handle<AddUiMaterial>>,
}

/// The info panels' scrollbar art (`GlueScrollFrameTemplate` + the CharacterCreate track pieces).
pub(crate) struct ScrollArt {
    pub(crate) up_btn: ScrollBtnArt,
    pub(crate) down_btn: ScrollBtnArt,
    pub(crate) knob: (Handle<Image>, Vec2),
    /// The decorative track behind the slider, shown only when the panel scrolls.
    pub(crate) track_top: Option<(Handle<Image>, Vec2)>,
    pub(crate) track_bottom: Option<(Handle<Image>, Vec2)>,
}

/// The create screen's reference art, loaded from the player's client data on first entry.
#[derive(Resource, Default)]
pub(crate) struct GlueArt {
    tried: bool,
    pub(crate) races: Option<(Handle<Image>, Vec2)>,
    pub(crate) classes: Option<(Handle<Image>, Vec2)>,
    pub(crate) gender: Option<(Handle<Image>, Vec2)>,
    pub(crate) factions: Option<(Handle<Image>, Vec2)>,
    pub(crate) banners: Option<Handle<Image>>,
    pub(crate) hilight: Option<Handle<AddUiMaterial>>,
    pub(crate) logo: Option<Handle<Image>>,
    pub(crate) arrow_left: Option<ArrowArt>,
    pub(crate) arrow_right: Option<ArrowArt>,
    pub(crate) button_up: Option<(Handle<Image>, Vec2)>,
    pub(crate) button_down: Option<(Handle<Image>, Vec2)>,
    pub(crate) button_dis: Option<(Handle<Image>, Vec2)>,
    pub(crate) button_hi: Option<Handle<AddUiMaterial>>,
    /// The tower frame: `UI-CharacterCreate-Background` (stretched) under three stacked
    /// `UI-CharacterCreate-OuterBorder` pieces.
    pub(crate) tower_bg: Option<Handle<Image>>,
    pub(crate) tower_border: Option<(Handle<Image>, Vec2)>,
    /// The 64² shadow behind every race/class/gender icon.
    pub(crate) icon_shadow: Option<Handle<Image>>,
    /// The dial rows' `CharacterCreate-LabelFrame`: 128×64, a 25|stretch|25 horizontal 3-slice.
    pub(crate) label_frame: Option<(Handle<Image>, Vec2)>,
    /// `UI-RotationRight-Big-*`; the left rotate button is this art mirrored.
    pub(crate) rotate_up: Option<Handle<Image>>,
    pub(crate) rotate_down: Option<Handle<Image>>,
    /// The rotate buttons' hover ring (`UI-Common-MouseHilight`).
    pub(crate) mouse_hilight: Option<Handle<AddUiMaterial>>,
    /// The two edge files split into their eight pieces, and the tiled `UI-Tooltip-Background`.
    pub(crate) panel_border: Option<BackdropEdges>,
    pub(crate) name_border: Option<BackdropEdges>,
    pub(crate) tooltip_bg: Option<Handle<Image>>,
    /// All or nothing: without the full set the info panels scroll by wheel only.
    pub(crate) scroll: Option<ScrollArt>,
    /// `Glue-CharacterSelect-Highlight` (ADD), on hover and on the locked selected row.
    pub(crate) select_highlight: Option<Handle<AddUiMaterial>>,
    /// The delete/rename dialog: `UI-DialogBox-Border` (edge 32) over `-Background`.
    pub(crate) dialog_border: Option<BackdropEdges>,
    pub(crate) dialog_bg: Option<Handle<Image>>,
    pub(crate) dialog_alert: Option<Handle<Image>>,
    /// The delete dialog's edit box (CharacterSelect.xml): two 75×32 pieces overhanging the
    /// 130-wide box by 10 each side.
    pub(crate) chat_input_left: Option<(Handle<Image>, Vec2)>,
    pub(crate) chat_input_right: Option<(Handle<Image>, Vec2)>,
    /// The login screen's logo (bottom center) and checkbox.
    pub(crate) blizzard_logo: Option<Handle<Image>>,
    pub(crate) checkbox: Option<CheckboxArt>,
    /// The AddOn List screen's set (`GlueXML/AddonList.xml`). The six `HelpFrame-*` pieces are the
    /// whole framed panel; its tooltip uses the in-game `UI-Tooltip-Border`, not the glue one.
    pub(crate) help_frame: Option<HelpFrameArt>,
    pub(crate) dialog_header: Option<(Handle<Image>, Vec2)>,
    pub(crate) close_btn: Option<CloseBtnArt>,
    pub(crate) dropdown_arrow_up: Option<Handle<Image>>,
    pub(crate) dropdown_arrow_down: Option<Handle<Image>>,
    pub(crate) quest_hilight: Option<Handle<AddUiMaterial>>,
    /// The realm list's selection band, tinted per row by
    /// `RealmListHighlightTexture:SetVertexColor`. Drawn alpha-blended where the reference uses
    /// `alphaMode="ADD"`: `AddUiMaterial` has no tint yet.
    pub(crate) title_highlight: Option<Handle<Image>>,
    pub(crate) check_disabled: Option<Handle<Image>>,
    pub(crate) tooltip_border: Option<BackdropEdges>,
    pub(crate) char_scrollbar: Option<(Handle<Image>, Vec2)>,

    /// The realm list's sort headers (`RealmSortButtonTemplate`, RealmList.xml).
    pub(crate) column_tabs: Option<(Handle<Image>, Vec2)>,
    pub(crate) tab_highlight: Option<Handle<AddUiMaterial>>,
    pub(crate) sort_arrow: Option<(Handle<Image>, Vec2)>,
}

/// `RealmSortButtonTemplate`'s `WhoFrame-ColumnTabs` slices, `[left, right, top, bottom]` each:
/// the left cap (5 wide), the stretched middle, the right cap (4 wide).
pub(crate) const COLUMN_TAB_TC: [[f32; 4]; 3] = [
    [0.0, 0.078_125, 0.0, 0.593_75],
    [0.078_125, 0.906_25, 0.0, 0.593_75],
    [0.906_25, 0.968_75, 0.0, 0.593_75],
];
/// `$parentArrow`'s texcoords into `UI-SortArrow`, the down-pointing half.
pub(crate) const SORT_ARROW_TC: [f32; 4] = [0.0, 0.5625, 0.0, 1.0];

/// The `HelpFrame-*` plate: six pieces tiling a 640×512 panel (TopLeft/Top 256², TopRight
/// 128×256 on top; BotLeft/Bottom/BotRight below).
pub(crate) struct HelpFrameArt {
    pub(crate) tl: Handle<Image>,
    pub(crate) top: Handle<Image>,
    pub(crate) tr: Handle<Image>,
    pub(crate) bl: Handle<Image>,
    pub(crate) bottom: Handle<Image>,
    pub(crate) br: Handle<Image>,
    /// Each piece's native size, for the divider strip's texcoord sub-rects.
    pub(crate) sizes: [Vec2; 3],
}

/// `GlueCloseButton` (GlueTemplates.xml): `UI-Panel-MinimizeButton-Up/Down/Highlight(ADD)`.
pub(crate) struct CloseBtnArt {
    pub(crate) up: Handle<Image>,
    pub(crate) down: Handle<Image>,
    pub(crate) hi: Option<Handle<AddUiMaterial>>,
}

impl GlueArt {
    /// Load the art set once; a piece that fails stays `None`.
    pub(crate) fn ensure_loaded(
        &mut self,
        assets: &mut WorldAssets,
        images: &mut Assets<Image>,
        add_mats: &mut Assets<AddUiMaterial>,
    ) {
        if self.tried {
            return;
        }
        self.tried = true;
        fn sized(
            assets: &mut WorldAssets,
            path: &str,
            images: &mut Assets<Image>,
        ) -> Option<(Handle<Image>, Vec2)> {
            let h = assets.sprite_texture(path, images)?;
            let size = images.get(&h)?.size_f32();
            Some((h, size))
        }
        fn arrow(
            assets: &mut WorldAssets,
            side: &str,
            images: &mut Assets<Image>,
            add_mats: &mut Assets<AddUiMaterial>,
        ) -> Option<ArrowArt> {
            let base = format!("Interface\\Glues\\Common\\Glue-{side}Arrow-Button-");
            Some(ArrowArt {
                up: assets.sprite_texture(&format!("{base}Up"), images)?,
                down: assets.sprite_texture(&format!("{base}Down"), images),
                hi: add_overlay(
                    assets,
                    &format!("{base}Highlight"),
                    FULL_TC,
                    images,
                    add_mats,
                ),
            })
        }
        const CC: &str = "Interface\\Glues\\CharacterCreate\\UI-CharacterCreate-";
        const GB: &str = "Interface\\Glues\\Common\\Glue-Panel-Button-";
        self.races = sized(assets, &format!("{CC}Races"), images);
        self.classes = sized(assets, &format!("{CC}Classes"), images);
        self.gender = sized(assets, &format!("{CC}Gender"), images);
        self.factions = sized(assets, &format!("{CC}Factions"), images);
        self.banners = assets.sprite_texture(&format!("{CC}Banners"), images);
        // No `CheckButtonHilight`: the template's `CheckedTexture` is commented out in 1.12
        // GlueXML, so the selected look is the locked highlight alone.
        self.hilight = add_overlay(
            assets,
            "Interface\\Buttons\\ButtonHilight-Square",
            FULL_TC,
            images,
            add_mats,
        );
        self.logo = assets.sprite_texture("Interface\\Glues\\Common\\Glues-WoW-Logo", images);
        self.arrow_left = arrow(assets, "Left", images, add_mats);
        self.arrow_right = arrow(assets, "Right", images, add_mats);
        self.button_up = sized(assets, &format!("{GB}Up"), images);
        self.button_down = sized(assets, &format!("{GB}Down"), images);
        self.button_dis = sized(assets, &format!("{GB}Disabled"), images);
        self.button_hi = add_overlay(
            assets,
            &format!("{GB}Highlight"),
            BUTTON_HI_TC,
            images,
            add_mats,
        );
        self.tower_bg = assets.sprite_texture(&format!("{CC}Background"), images);
        self.tower_border = sized(assets, &format!("{CC}OuterBorder"), images);
        self.icon_shadow = assets.sprite_texture(&format!("{CC}IconShadow"), images);
        self.label_frame = sized(
            assets,
            "Interface\\Glues\\CharacterCreate\\CharacterCreate-LabelFrame",
            images,
        );
        self.rotate_up = assets.sprite_texture(
            "Interface\\Glues\\CharacterCreate\\UI-RotationRight-Big-Up",
            images,
        );
        self.rotate_down = assets.sprite_texture(
            "Interface\\Glues\\CharacterCreate\\UI-RotationRight-Big-Down",
            images,
        );
        self.mouse_hilight = add_overlay(
            assets,
            "Interface\\Buttons\\UI-Common-MouseHilight",
            FULL_TC,
            images,
            add_mats,
        );
        self.panel_border = super::backdrop::backdrop_edges(
            assets,
            "Interface\\Glues\\Common\\TextPanel-Border",
            images,
        );
        self.name_border = super::backdrop::backdrop_edges(
            assets,
            "Interface\\Glues\\Common\\Glue-Tooltip-Border",
            images,
        );
        self.tooltip_bg =
            assets.sprite_texture("Interface\\Tooltips\\UI-Tooltip-Background", images);
        fn scroll_btn(
            assets: &mut WorldAssets,
            dir: &str,
            images: &mut Assets<Image>,
            add_mats: &mut Assets<AddUiMaterial>,
        ) -> Option<ScrollBtnArt> {
            let base = format!("Interface\\Buttons\\UI-ScrollBar-Scroll{dir}Button-");
            let h = assets.sprite_texture(&format!("{base}Up"), images)?;
            let size = images.get(&h)?.size_f32();
            Some(ScrollBtnArt {
                up: h,
                down: assets.sprite_texture(&format!("{base}Down"), images)?,
                dis: assets.sprite_texture(&format!("{base}Disabled"), images)?,
                hi: add_overlay(
                    assets,
                    &format!("{base}Highlight"),
                    SCROLL_BTN_TC,
                    images,
                    add_mats,
                )?,
                size,
            })
        }
        self.scroll = (|| {
            Some(ScrollArt {
                up_btn: scroll_btn(assets, "Up", images, add_mats)?,
                down_btn: scroll_btn(assets, "Down", images, add_mats)?,
                knob: sized(assets, "Interface\\Buttons\\UI-ScrollBar-Knob", images)?,
                track_top: sized(assets, &format!("{CC}ScrollBar-Top"), images),
                track_bottom: sized(
                    assets,
                    "Interface\\ClassTrainerFrame\\UI-ClassTrainer-ScrollBar",
                    images,
                ),
            })
        })();
        // The select screen's set.
        self.select_highlight = add_overlay(
            assets,
            "Interface\\Glues\\CharacterSelect\\Glue-CharacterSelect-Highlight",
            FULL_TC,
            images,
            add_mats,
        );
        self.dialog_border = super::backdrop::backdrop_edges(
            assets,
            "Interface\\DialogFrame\\UI-DialogBox-Border",
            images,
        );
        self.dialog_bg =
            assets.sprite_texture("Interface\\DialogFrame\\UI-DialogBox-Background", images);
        self.dialog_alert =
            assets.sprite_texture("Interface\\DialogFrame\\DialogAlertIcon", images);
        self.chat_input_left = sized(
            assets,
            "Interface\\ChatFrame\\UI-ChatInputBorder-Left",
            images,
        );
        self.chat_input_right = sized(
            assets,
            "Interface\\ChatFrame\\UI-ChatInputBorder-Right",
            images,
        );
        // The login screen's set.
        self.blizzard_logo =
            assets.sprite_texture("Interface\\Glues\\Mainmenu\\Glues-BlizzardLogo", images);
        self.checkbox = (|| {
            Some(CheckboxArt {
                up: assets.sprite_texture("Interface\\Buttons\\UI-CheckBox-Up", images)?,
                down: assets.sprite_texture("Interface\\Buttons\\UI-CheckBox-Down", images)?,
                checked: assets.sprite_texture("Interface\\Buttons\\UI-CheckBox-Check", images)?,
                hi: add_overlay(
                    assets,
                    "Interface\\Buttons\\UI-CheckBox-Highlight",
                    FULL_TC,
                    images,
                    add_mats,
                ),
            })
        })();
        // The AddOn List screen's set.
        const HF: &str = "Interface\\HelpFrame\\HelpFrame-";
        self.help_frame = (|| {
            let tl = sized(assets, &format!("{HF}TopLeft"), images)?;
            let top = sized(assets, &format!("{HF}Top"), images)?;
            let tr = sized(assets, &format!("{HF}TopRight"), images)?;
            Some(HelpFrameArt {
                sizes: [tl.1, top.1, tr.1],
                tl: tl.0,
                top: top.0,
                tr: tr.0,
                bl: assets.sprite_texture(&format!("{HF}BotLeft"), images)?,
                bottom: assets.sprite_texture(&format!("{HF}Bottom"), images)?,
                br: assets.sprite_texture(&format!("{HF}BotRight"), images)?,
            })
        })();
        self.dialog_header = sized(
            assets,
            "Interface\\DialogFrame\\UI-DialogBox-Header",
            images,
        );
        self.close_btn = (|| {
            const MB: &str = "Interface\\Buttons\\UI-Panel-MinimizeButton-";
            Some(CloseBtnArt {
                up: assets.sprite_texture(&format!("{MB}Up"), images)?,
                down: assets.sprite_texture(&format!("{MB}Down"), images)?,
                hi: add_overlay(assets, &format!("{MB}Highlight"), FULL_TC, images, add_mats),
            })
        })();
        self.dropdown_arrow_up =
            assets.sprite_texture("Interface\\ChatFrame\\UI-ChatIcon-ScrollDown-Up", images);
        self.dropdown_arrow_down =
            assets.sprite_texture("Interface\\ChatFrame\\UI-ChatIcon-ScrollDown-Down", images);
        self.quest_hilight = add_overlay(
            assets,
            "Interface\\QuestFrame\\UI-QuestTitleHighlight",
            FULL_TC,
            images,
            add_mats,
        );
        self.title_highlight =
            assets.sprite_texture("Interface\\QuestFrame\\UI-QuestLogTitleHighlight", images);
        self.check_disabled =
            assets.sprite_texture("Interface\\Buttons\\UI-CheckBox-Check-Disabled", images);
        self.tooltip_border = super::backdrop::backdrop_edges(
            assets,
            "Interface\\Tooltips\\UI-Tooltip-Border",
            images,
        );
        self.char_scrollbar = sized(
            assets,
            "Interface\\PaperDollInfoFrame\\UI-Character-ScrollBar",
            images,
        );
        self.column_tabs = sized(
            assets,
            "Interface\\FriendsFrame\\WhoFrame-ColumnTabs",
            images,
        );
        self.tab_highlight = add_overlay(
            assets,
            "Interface\\PaperDollInfoFrame\\UI-Character-Tab-Highlight",
            FULL_TC,
            images,
            add_mats,
        );
        self.sort_arrow = sized(assets, "Interface\\Buttons\\UI-SortArrow", images);
        debug!(
            "glue art: addonlist set — helpframe {} header {} close {} droparrow {}/{} \
             questhl {} titlehl {} greycheck {} tipborder {} scrolltrack {} \
             coltabs {} tabhl {} sortarrow {}",
            self.help_frame.is_some(),
            self.dialog_header.is_some(),
            self.close_btn.is_some(),
            self.dropdown_arrow_up.is_some(),
            self.dropdown_arrow_down.is_some(),
            self.quest_hilight.is_some(),
            self.title_highlight.is_some(),
            self.check_disabled.is_some(),
            self.tooltip_border.is_some(),
            self.char_scrollbar.is_some(),
            self.column_tabs.is_some(),
            self.tab_highlight.is_some(),
            self.sort_arrow.is_some(),
        );
        debug!(
            "glue art: races {} classes {} gender {} factions {} banners {} hilight {} logo {} \
             arrows {}/{} button {}/{}/{}/{} tower {}/{} shadow {} label {} rotate {}/{} \
             mousehl {} borders {}/{} bg {} scroll {} sel-hi {} dialog {}/{}/{} chatinput {}/{} \
             blizz {} check {}",
            self.races.is_some(),
            self.classes.is_some(),
            self.gender.is_some(),
            self.factions.is_some(),
            self.banners.is_some(),
            self.hilight.is_some(),
            self.logo.is_some(),
            self.arrow_left.is_some(),
            self.arrow_right.is_some(),
            self.button_up.is_some(),
            self.button_down.is_some(),
            self.button_dis.is_some(),
            self.button_hi.is_some(),
            self.tower_bg.is_some(),
            self.tower_border.is_some(),
            self.icon_shadow.is_some(),
            self.label_frame.is_some(),
            self.rotate_up.is_some(),
            self.rotate_down.is_some(),
            self.mouse_hilight.is_some(),
            self.panel_border.is_some(),
            self.name_border.is_some(),
            self.tooltip_bg.is_some(),
            self.scroll.is_some(),
            self.select_highlight.is_some(),
            self.dialog_border.is_some(),
            self.dialog_bg.is_some(),
            self.dialog_alert.is_some(),
            self.chat_input_left.is_some(),
            self.chat_input_right.is_some(),
            self.blizzard_logo.is_some(),
            self.checkbox.is_some(),
        );
    }
}

// ── WoW `alphaMode="ADD"` overlays ───────────────────────────────────────────────────────────────

/// The whole-texture uv rect for [`add_overlay`].
const FULL_TC: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

/// One `alphaMode="ADD"` highlight (a glow authored on opaque black) as an [`AddUiMaterial`];
/// `tc` is the `[left, right, top, bottom]` texcoord region.
fn add_overlay(
    assets: &mut WorldAssets,
    path: &str,
    tc: [f32; 4],
    images: &mut Assets<Image>,
    add_mats: &mut Assets<AddUiMaterial>,
) -> Option<Handle<AddUiMaterial>> {
    let texture = assets.sprite_texture(path, images)?;
    Some(add_mats.add(AddUiMaterial {
        texture,
        rect: Vec4::new(tc[0], tc[2], tc[1], tc[3]),
    }))
}

// ── The icon-cell tables (CharacterCreate.lua's *_ICON_TCOORDS, verbatim) ────────────────────────

/// A race's cell in `UI-CharacterCreate-Races` (col, row; female is row + 2), `RACE_ICON_TCOORDS`.
fn race_cell(race: u8) -> Option<(f32, f32)> {
    Some(match race {
        1 => (0.0, 0.0), // Human
        3 => (1.0, 0.0), // Dwarf
        7 => (2.0, 0.0), // Gnome
        4 => (3.0, 0.0), // Night Elf
        6 => (0.0, 1.0), // Tauren
        5 => (1.0, 1.0), // Scourge
        8 => (2.0, 1.0), // Troll
        2 => (3.0, 1.0), // Orc
        _ => return None,
    })
}

pub(crate) fn race_tc(race: u8, sex: u8) -> Option<[f32; 4]> {
    let (c, r) = race_cell(race)?;
    let r = r + if sex == 1 { 2.0 } else { 0.0 };
    Some([c * 0.25, (c + 1.0) * 0.25, r * 0.25, (r + 1.0) * 0.25])
}

/// A class icon's texcoords in `UI-CharacterCreate-Classes`, `CLASS_ICON_TCOORDS` verbatim.
pub(crate) fn class_tc(class: u8) -> Option<[f32; 4]> {
    Some(match class {
        1 => [0.0, 0.25, 0.0, 0.25],              // Warrior
        8 => [0.25, 0.49609375, 0.0, 0.25],       // Mage
        4 => [0.49609375, 0.7421875, 0.0, 0.25],  // Rogue
        11 => [0.7421875, 0.98828125, 0.0, 0.25], // Druid
        3 => [0.0, 0.25, 0.25, 0.5],              // Hunter
        7 => [0.25, 0.49609375, 0.25, 0.5],       // Shaman
        5 => [0.49609375, 0.7421875, 0.25, 0.5],  // Priest
        9 => [0.7421875, 0.98828125, 0.25, 0.5],  // Warlock
        2 => [0.0, 0.25, 0.5, 0.75],              // Paladin
        _ => return None,
    })
}

/// `GlueButtons.xml` texcoords: Up/Down/Disabled share one region, the Highlight has its own.
pub(crate) const BUTTON_TC: [f32; 4] = [0.0, 0.578125, 0.0, 0.75];
pub(crate) const BUTTON_HI_TC: [f32; 4] = [0.0, 0.625, 0.0, 0.6875];
/// The scrollbar buttons and knob sit in the center quarter of their sheets.
pub(crate) const SCROLL_BTN_TC: [f32; 4] = [0.25, 0.75, 0.25, 0.75];

/// Texcoords to an `ImageNode` pixel rect on a texture of `size`.
pub(crate) fn tc_rect(size: Vec2, tc: [f32; 4]) -> Rect {
    Rect::new(
        tc[0] * size.x,
        tc[2] * size.y,
        tc[1] * size.x,
        tc[3] * size.y,
    )
}
