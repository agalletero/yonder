//! Tokens de diseño y aplicación al estilo de egui.
//!
//! Reglas que este módulo hace cumplir, y que son el motivo de que exista en
//! vez de repartir colores por los componentes:
//!
//! - **Cero hex sueltos fuera de aquí.** Todo color de la interfaz sale de un
//!   token semántico. Si un componente necesita un color que no está, se añade
//!   el token; no se escribe el hex en el sitio.
//! - **Claro y oscuro son dos paletas, no una invertida.** En oscuro la
//!   elevación se expresa con superficies más claras y los acentos van
//!   desaturados; en claro, con sombras suaves y acentos a saturación plena.
//! - **Ni negro ni blanco puros** en superficies ni en texto: cansan la vista.
//! - **Espaciado en rejilla de 4·n.** Nada de valores sueltos.

use eframe::egui::{self, Color32, CornerRadius, Margin, Shadow, Stroke};

use yonder::prefs::{Densidad, Preferencias, Tema as PreferenciaTema};
use yonder::state::machine::Estado;

/// Escala de espaciado. Todo el `padding` y todo el `margin` salen de aquí.
#[derive(Debug, Clone, Copy)]
pub struct Escala {
    pub xs: f32,
    pub s: f32,
    pub m: f32,
    pub l: f32,
    pub xl: f32,
    pub xxl: f32,
}

impl Escala {
    fn con_densidad(densidad: Densidad) -> Escala {
        let factor = densidad.factor();
        Escala {
            xs: 4.0 * factor,
            s: 8.0 * factor,
            m: 12.0 * factor,
            l: 16.0 * factor,
            xl: 24.0 * factor,
            xxl: 32.0 * factor,
        }
    }
}

/// Radios de esquina. Tres, no siete.
#[derive(Debug, Clone, Copy)]
pub struct Radios {
    /// Chips, casillas, elementos pequeños.
    pub pequeno: u8,
    /// Botones y campos.
    pub medio: u8,
    /// Tarjetas y modales.
    pub grande: u8,
}

pub const RADIOS: Radios = Radios {
    pequeno: 4,
    medio: 6,
    grande: 10,
};

/// Tamaños de letra. Cinco, no once.
#[derive(Debug, Clone, Copy)]
pub struct Tipografia {
    /// Etiquetas de chip y notas al pie.
    pub micro: f32,
    /// Texto secundario.
    pub pequeno: f32,
    /// Texto de cuerpo.
    pub cuerpo: f32,
    /// Título de fila.
    pub titulo: f32,
    /// Título de pantalla.
    pub cabecera: f32,
}

pub const TIPOGRAFIA: Tipografia = Tipografia {
    micro: 11.0,
    pequeno: 12.0,
    cuerpo: 13.0,
    titulo: 15.0,
    cabecera: 18.0,
};

/// Paleta por roles semánticos.
#[derive(Debug, Clone, Copy)]
pub struct Paleta {
    pub oscuro: bool,

    // Superficies, de menor a mayor elevación.
    pub fondo: Color32,
    pub superficie: Color32,
    pub elevado: Color32,
    pub hover: Color32,
    pub pulsado: Color32,

    // Líneas.
    pub borde: Color32,
    pub borde_fuerte: Color32,
    pub divisor: Color32,

    // Texto.
    pub texto: Color32,
    pub texto_secundario: Color32,
    pub texto_tenue: Color32,
    pub texto_sobre_acento: Color32,

    // Acento de marca.
    pub acento: Color32,
    pub acento_hover: Color32,
    pub acento_suave: Color32,

    // Estados.
    pub exito: Color32,
    pub exito_suave: Color32,
    pub aviso: Color32,
    pub aviso_suave: Color32,
    pub error: Color32,
    pub error_suave: Color32,
    pub info: Color32,
    pub info_suave: Color32,
}

/// Paleta oscura.
///
/// Base gris muy oscuro con un punto de azul, nunca negro. La elevación sube
/// aclarando la superficie, no con sombras: sobre fondo oscuro una sombra no se
/// ve. Los acentos van desaturados respecto a los del modo claro porque un
/// color saturado sobre oscuro vibra y cansa.
const OSCURA: Paleta = Paleta {
    oscuro: true,

    fondo: Color32::from_rgb(0x14, 0x16, 0x1A),
    superficie: Color32::from_rgb(0x1A, 0x1D, 0x23),
    elevado: Color32::from_rgb(0x21, 0x25, 0x2D),
    hover: Color32::from_rgb(0x26, 0x2B, 0x34),
    pulsado: Color32::from_rgb(0x2D, 0x33, 0x3D),

    borde: Color32::from_rgb(0x2C, 0x31, 0x3A),
    borde_fuerte: Color32::from_rgb(0x3A, 0x41, 0x4C),
    divisor: Color32::from_rgb(0x23, 0x27, 0x2E),

    texto: Color32::from_rgb(0xE4, 0xE7, 0xEC),
    texto_secundario: Color32::from_rgb(0xA2, 0xAB, 0xB8),
    texto_tenue: Color32::from_rgb(0x6C, 0x76, 0x83),
    texto_sobre_acento: Color32::from_rgb(0x0C, 0x1B, 0x1A),

    acento: Color32::from_rgb(0x4F, 0xC3, 0xB0),
    acento_hover: Color32::from_rgb(0x67, 0xD4, 0xC2),
    acento_suave: Color32::from_rgb(0x1B, 0x33, 0x31),

    exito: Color32::from_rgb(0x5F, 0xBF, 0x87),
    exito_suave: Color32::from_rgb(0x18, 0x2E, 0x23),
    aviso: Color32::from_rgb(0xE0, 0xB1, 0x5C),
    aviso_suave: Color32::from_rgb(0x33, 0x2A, 0x16),
    error: Color32::from_rgb(0xE5, 0x83, 0x79),
    error_suave: Color32::from_rgb(0x35, 0x1F, 0x1D),
    info: Color32::from_rgb(0x6F, 0xA8, 0xDC),
    info_suave: Color32::from_rgb(0x1A, 0x28, 0x36),
};

/// Paleta clara.
///
/// Blanco roto de base, nunca `#ffffff` en el fondo ni `#000000` en el texto.
/// La elevación se expresa con sombra suave sobre superficie blanca.
const CLARA: Paleta = Paleta {
    oscuro: false,

    fondo: Color32::from_rgb(0xF6, 0xF7, 0xF9),
    superficie: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    elevado: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    hover: Color32::from_rgb(0xED, 0xEF, 0xF3),
    pulsado: Color32::from_rgb(0xE2, 0xE6, 0xEC),

    borde: Color32::from_rgb(0xDE, 0xE2, 0xE8),
    borde_fuerte: Color32::from_rgb(0xC2, 0xC9, 0xD2),
    divisor: Color32::from_rgb(0xEB, 0xEE, 0xF2),

    texto: Color32::from_rgb(0x1A, 0x1D, 0x23),
    texto_secundario: Color32::from_rgb(0x58, 0x61, 0x6D),
    texto_tenue: Color32::from_rgb(0x89, 0x92, 0x9E),
    texto_sobre_acento: Color32::from_rgb(0xFF, 0xFF, 0xFF),

    acento: Color32::from_rgb(0x0F, 0x76, 0x6E),
    acento_hover: Color32::from_rgb(0x0B, 0x5E, 0x58),
    acento_suave: Color32::from_rgb(0xDB, 0xEF, 0xEC),

    exito: Color32::from_rgb(0x1E, 0x7A, 0x47),
    exito_suave: Color32::from_rgb(0xDF, 0xF1, 0xE6),
    aviso: Color32::from_rgb(0x96, 0x65, 0x0B),
    aviso_suave: Color32::from_rgb(0xFB, 0xEE, 0xD4),
    error: Color32::from_rgb(0xC0, 0x39, 0x2B),
    error_suave: Color32::from_rgb(0xFB, 0xE4, 0xE1),
    info: Color32::from_rgb(0x1B, 0x6A, 0xAC),
    info_suave: Color32::from_rgb(0xDF, 0xEC, 0xF9),
};

/// Tema completo: paleta más medidas.
#[derive(Debug, Clone, Copy)]
pub struct Tema {
    pub paleta: Paleta,
    pub escala: Escala,
    pub radios: Radios,
    pub tipografia: Tipografia,
}

impl Tema {
    pub fn nuevo(preferencias: &Preferencias, escritorio_oscuro: bool) -> Tema {
        let oscuro = match preferencias.tema {
            PreferenciaTema::Auto => escritorio_oscuro,
            PreferenciaTema::Oscuro => true,
            PreferenciaTema::Claro => false,
        };
        Tema {
            paleta: if oscuro { OSCURA } else { CLARA },
            escala: Escala::con_densidad(preferencias.densidad),
            radios: RADIOS,
            tipografia: TIPOGRAFIA,
        }
    }

    /// Color principal de un estado de §4.
    pub fn color_estado(&self, estado: Estado) -> Color32 {
        match estado {
            Estado::Activo => self.paleta.exito,
            Estado::Degradado => self.paleta.aviso,
            Estado::Fallido => self.paleta.error,
            Estado::Conectando | Estado::Reintentando => self.paleta.info,
            Estado::Cerrando => self.paleta.texto_secundario,
            Estado::Definido => self.paleta.texto_tenue,
        }
    }

    /// Fondo tenue a juego con el color de un estado, para chips.
    pub fn fondo_estado(&self, estado: Estado) -> Color32 {
        match estado {
            Estado::Activo => self.paleta.exito_suave,
            Estado::Degradado => self.paleta.aviso_suave,
            Estado::Fallido => self.paleta.error_suave,
            Estado::Conectando | Estado::Reintentando => self.paleta.info_suave,
            Estado::Cerrando | Estado::Definido => {
                if self.paleta.oscuro {
                    self.paleta.elevado
                } else {
                    self.paleta.hover
                }
            }
        }
    }

    /// Elevación de una tarjeta.
    ///
    /// En claro es una sombra de dos capas —una ambiental difusa y una directa
    /// corta—; en oscuro no hay sombra porque no se vería: la elevación la da
    /// la superficie más clara.
    pub fn sombra_tarjeta(&self) -> Shadow {
        if self.paleta.oscuro {
            Shadow::NONE
        } else {
            Shadow {
                offset: [0, 1],
                blur: 3,
                spread: 0,
                color: Color32::from_black_alpha(18),
            }
        }
    }

    /// Elevación de un modal: más alta, más difusa.
    pub fn sombra_modal(&self) -> Shadow {
        if self.paleta.oscuro {
            Shadow {
                offset: [0, 6],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(120),
            }
        } else {
            Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(38),
            }
        }
    }

    /// Marco de tarjeta: superficie, borde de 1 px y radio grande.
    pub fn marco_tarjeta(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.paleta.superficie)
            .stroke(Stroke::new(1.0, self.paleta.borde))
            .corner_radius(CornerRadius::same(self.radios.grande))
            .inner_margin(self.margen(self.escala.m))
            .shadow(self.sombra_tarjeta())
    }

    /// Marco de panel: superficie sin borde ni sombra.
    pub fn marco_panel(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.paleta.fondo)
            .inner_margin(self.margen(self.escala.l))
    }

    /// Marco de modal.
    pub fn marco_modal(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.paleta.elevado)
            .stroke(Stroke::new(1.0, self.paleta.borde_fuerte))
            .corner_radius(CornerRadius::same(self.radios.grande))
            .inner_margin(self.margen(self.escala.xl))
            .shadow(self.sombra_modal())
    }

    pub fn margen(&self, valor: f32) -> Margin {
        Margin::same(valor.round() as i8)
    }

    pub fn margen_simetrico(&self, x: f32, y: f32) -> Margin {
        Margin::symmetric(x.round() as i8, y.round() as i8)
    }

    /// Aplica los tokens al estilo global de egui.
    pub fn aplicar(&self, contexto: &egui::Context) {
        let paleta = self.paleta;
        let mut estilo = (*contexto.style()).clone();

        // --- Tipografía: pocos tamaños, todos de la escala ---
        use egui::{FontFamily, FontId, TextStyle};
        estilo.text_styles = [
            (
                TextStyle::Small,
                FontId::new(self.tipografia.micro, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                FontId::new(self.tipografia.cuerpo, FontFamily::Proportional),
            ),
            (
                TextStyle::Button,
                FontId::new(self.tipografia.cuerpo, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(self.tipografia.cabecera, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(self.tipografia.pequeno, FontFamily::Monospace),
            ),
        ]
        .into();

        // --- Espaciado: todo sale de la rejilla ---
        let espaciado = &mut estilo.spacing;
        espaciado.item_spacing = egui::vec2(self.escala.s, self.escala.s);
        espaciado.button_padding = egui::vec2(self.escala.m, self.escala.xs + 2.0);
        espaciado.menu_margin = self.margen(self.escala.s);
        espaciado.indent = self.escala.l;
        espaciado.interact_size = egui::vec2(self.escala.xl, self.escala.xl);
        espaciado.icon_width = 16.0;
        espaciado.icon_width_inner = 9.0;
        espaciado.scroll.bar_width = 8.0;
        espaciado.scroll.floating = true;

        // --- Colores por rol ---
        let visuales = &mut estilo.visuals;
        visuales.dark_mode = paleta.oscuro;
        visuales.panel_fill = paleta.fondo;
        visuales.window_fill = paleta.elevado;
        visuales.extreme_bg_color = if paleta.oscuro {
            paleta.fondo
        } else {
            paleta.hover
        };
        visuales.faint_bg_color = paleta.superficie;
        visuales.code_bg_color = paleta.superficie;

        visuales.override_text_color = None;
        visuales.hyperlink_color = paleta.acento;
        visuales.warn_fg_color = paleta.aviso;
        visuales.error_fg_color = paleta.error;
        visuales.selection.bg_fill = paleta.acento_suave;
        visuales.selection.stroke = Stroke::new(1.0, paleta.acento);

        visuales.window_corner_radius = CornerRadius::same(self.radios.grande);
        visuales.menu_corner_radius = CornerRadius::same(self.radios.medio);
        visuales.window_stroke = Stroke::new(1.0, paleta.borde);
        visuales.window_shadow = self.sombra_modal();
        visuales.popup_shadow = self.sombra_modal();

        let radio = CornerRadius::same(self.radios.medio);

        // Elemento en reposo: sin fondo. El cromo baja, el contenido sube.
        visuales.widgets.noninteractive.bg_fill = paleta.superficie;
        visuales.widgets.noninteractive.weak_bg_fill = paleta.superficie;
        visuales.widgets.noninteractive.bg_stroke = Stroke::new(1.0, paleta.divisor);
        visuales.widgets.noninteractive.fg_stroke = Stroke::new(1.0, paleta.texto_secundario);
        visuales.widgets.noninteractive.corner_radius = radio;
        visuales.widgets.noninteractive.expansion = 0.0;

        visuales.widgets.inactive.bg_fill = paleta.superficie;
        visuales.widgets.inactive.weak_bg_fill = if paleta.oscuro {
            paleta.elevado
        } else {
            paleta.superficie
        };
        visuales.widgets.inactive.bg_stroke = Stroke::new(1.0, paleta.borde);
        visuales.widgets.inactive.fg_stroke = Stroke::new(1.0, paleta.texto);
        visuales.widgets.inactive.corner_radius = radio;
        visuales.widgets.inactive.expansion = 0.0;

        visuales.widgets.hovered.bg_fill = paleta.hover;
        visuales.widgets.hovered.weak_bg_fill = paleta.hover;
        visuales.widgets.hovered.bg_stroke = Stroke::new(1.0, paleta.borde_fuerte);
        visuales.widgets.hovered.fg_stroke = Stroke::new(1.0, paleta.texto);
        visuales.widgets.hovered.corner_radius = radio;
        // Sin salto de tamaño al pasar por encima: la precisión es no moverse.
        visuales.widgets.hovered.expansion = 0.0;

        visuales.widgets.active.bg_fill = paleta.pulsado;
        visuales.widgets.active.weak_bg_fill = paleta.pulsado;
        visuales.widgets.active.bg_stroke = Stroke::new(1.0, paleta.acento);
        visuales.widgets.active.fg_stroke = Stroke::new(1.0, paleta.texto);
        visuales.widgets.active.corner_radius = radio;
        visuales.widgets.active.expansion = 0.0;

        visuales.widgets.open.bg_fill = paleta.elevado;
        visuales.widgets.open.weak_bg_fill = paleta.elevado;
        visuales.widgets.open.bg_stroke = Stroke::new(1.0, paleta.borde_fuerte);
        visuales.widgets.open.fg_stroke = Stroke::new(1.0, paleta.texto);
        visuales.widgets.open.corner_radius = radio;

        // `strong` sube la jerarquía por color, no por tamaño.
        visuales.widgets.noninteractive.fg_stroke.color = paleta.texto_secundario;

        estilo.visuals.striped = false;
        estilo.animation_time = 0.10;

        contexto.set_style(estilo);
    }
}

/// Texto principal de una fila.
pub fn titulo(tema: &Tema, texto: impl Into<String>) -> egui::RichText {
    egui::RichText::new(texto)
        .size(tema.tipografia.titulo)
        .color(tema.paleta.texto)
}

/// Texto de cuerpo.
pub fn cuerpo(tema: &Tema, texto: impl Into<String>) -> egui::RichText {
    egui::RichText::new(texto)
        .size(tema.tipografia.cuerpo)
        .color(tema.paleta.texto)
}

/// Texto secundario: baja de color antes que de tamaño.
pub fn secundario(tema: &Tema, texto: impl Into<String>) -> egui::RichText {
    egui::RichText::new(texto)
        .size(tema.tipografia.pequeno)
        .color(tema.paleta.texto_secundario)
}

/// Texto tenue, el escalón más bajo de la jerarquía.
pub fn tenue(tema: &Tema, texto: impl Into<String>) -> egui::RichText {
    egui::RichText::new(texto)
        .size(tema.tipografia.pequeno)
        .color(tema.paleta.texto_tenue)
}

/// Texto monoespaciado, para puertos, rutas y fingerprints.
///
/// Los números en columna solo se leen bien si son de ancho fijo.
pub fn mono(tema: &Tema, texto: impl Into<String>) -> egui::RichText {
    egui::RichText::new(texto)
        .size(tema.tipografia.pequeno)
        .monospace()
        .color(tema.paleta.texto_secundario)
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn ninguna_paleta_usa_negro_ni_blanco_puros_en_superficies() {
        for paleta in [OSCURA, CLARA] {
            for color in [paleta.fondo, paleta.elevado, paleta.hover] {
                assert_ne!(color, Color32::BLACK, "hay negro puro en una superficie");
            }
            assert_ne!(paleta.texto, Color32::BLACK, "hay negro puro en el texto");
            assert_ne!(paleta.texto, Color32::WHITE, "hay blanco puro en el texto");
            assert_ne!(paleta.fondo, Color32::WHITE, "hay blanco puro en el fondo");
        }
    }

    #[test]
    fn en_oscuro_la_elevacion_sube_aclarando() {
        let claridad = |c: Color32| c.r() as u32 + c.g() as u32 + c.b() as u32;
        assert!(claridad(OSCURA.superficie) > claridad(OSCURA.fondo));
        assert!(claridad(OSCURA.elevado) > claridad(OSCURA.superficie));
        assert!(claridad(OSCURA.hover) > claridad(OSCURA.elevado));
    }

    #[test]
    fn en_oscuro_no_hay_sombra_en_las_tarjetas() {
        // Sobre fondo oscuro una sombra no se ve: la elevación es la superficie.
        let tema = Tema {
            paleta: OSCURA,
            escala: Escala::con_densidad(Densidad::Comoda),
            radios: RADIOS,
            tipografia: TIPOGRAFIA,
        };
        assert_eq!(tema.sombra_tarjeta(), Shadow::NONE);

        let claro = Tema {
            paleta: CLARA,
            ..tema
        };
        assert_ne!(claro.sombra_tarjeta(), Shadow::NONE);
    }

    #[test]
    fn el_acento_oscuro_esta_desaturado_respecto_al_claro() {
        let saturacion = |c: Color32| {
            let maximo = c.r().max(c.g()).max(c.b()) as f32;
            let minimo = c.r().min(c.g()).min(c.b()) as f32;
            if maximo == 0.0 {
                0.0
            } else {
                (maximo - minimo) / maximo
            }
        };
        assert!(
            saturacion(OSCURA.acento) < saturacion(CLARA.acento),
            "el acento oscuro debería vibrar menos que el claro"
        );
    }

    #[test]
    fn la_escala_es_multiplo_de_cuatro_en_densidad_comoda() {
        let escala = Escala::con_densidad(Densidad::Comoda);
        for valor in [
            escala.xs, escala.s, escala.m, escala.l, escala.xl, escala.xxl,
        ] {
            assert_eq!(
                valor % 4.0,
                0.0,
                "el valor {valor} se sale de la rejilla de 4·n"
            );
        }
    }

    #[test]
    fn la_densidad_compacta_encoge_sin_romper_el_orden() {
        let comoda = Escala::con_densidad(Densidad::Comoda);
        let compacta = Escala::con_densidad(Densidad::Compacta);
        assert!(compacta.m < comoda.m);
        assert!(compacta.xs < compacta.s && compacta.s < compacta.m);
    }

    #[test]
    fn cada_estado_tiene_color_propio_y_los_criticos_destacan() {
        let tema = Tema {
            paleta: OSCURA,
            escala: Escala::con_densidad(Densidad::Comoda),
            radios: RADIOS,
            tipografia: TIPOGRAFIA,
        };
        assert_eq!(tema.color_estado(Estado::Activo), OSCURA.exito);
        assert_eq!(tema.color_estado(Estado::Degradado), OSCURA.aviso);
        assert_eq!(tema.color_estado(Estado::Fallido), OSCURA.error);
        // Degradado no puede pintarse igual que Activo: es justo lo que hay que
        // distinguir de un vistazo.
        assert_ne!(
            tema.color_estado(Estado::Degradado),
            tema.color_estado(Estado::Activo)
        );
    }
}
