//! Piezas visuales reutilizables.
//!
//! Todas siguen la misma regla: el color sale de un token, el espaciado de la
//! rejilla, y nada crece ni salta al pasar el ratón por encima. Un elemento que
//! se mueve al hacer *hover* desalinea todo lo que tiene al lado.

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind, Vec2};

use yonder::state::machine::Estado;

use super::iconos::{self, Icono};
use super::tema::Tema;

/// Chip: etiqueta compacta con icono opcional y fondo tenue.
///
/// Es el patrón que resuelve «metadato secundario» en toda la interfaz: saltos,
/// llave física, autoarranque, origen externo. Uno solo, usado en todas partes.
pub fn chip(
    ui: &mut egui::Ui,
    tema: &Tema,
    icono: Option<Icono>,
    texto: &str,
    color: Color32,
    fondo: Color32,
) -> egui::Response {
    egui::Frame::new()
        .fill(fondo)
        .corner_radius(CornerRadius::same(tema.radios.pequeno))
        .inner_margin(tema.margen_simetrico(tema.escala.s, 2.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.xs;
            ui.horizontal(|ui| {
                if let Some(icono) = icono {
                    iconos::mostrar(ui, icono, iconos::PEQUENO, color);
                }
                ui.label(
                    egui::RichText::new(texto)
                        .size(tema.tipografia.micro)
                        .color(color),
                );
            });
        })
        .response
}

/// Chip neutro, para metadatos que no son ni buenos ni malos.
pub fn chip_neutro(ui: &mut egui::Ui, tema: &Tema, icono: Icono, texto: &str) -> egui::Response {
    let fondo = if tema.paleta.oscuro {
        tema.paleta.elevado
    } else {
        tema.paleta.hover
    };
    chip(
        ui,
        tema,
        Some(icono),
        texto,
        tema.paleta.texto_secundario,
        fondo,
    )
}

/// Indicador de estado: punto de color más icono, con el color del token.
///
/// En tránsito el icono gira. Un estado que dice «Conectando» sin moverse
/// parece un estado atascado, y el usuario acaba dándole otra vez al botón.
pub fn indicador_estado(ui: &mut egui::Ui, tema: &Tema, estado: Estado, tamano: f32) {
    let color = tema.color_estado(estado);
    match estado {
        Estado::Conectando => {
            iconos::mostrar_girando(ui, Icono::CONECTANDO, tamano, color, 1.0);
        }
        Estado::Reintentando => {
            iconos::mostrar_girando(ui, Icono::REINTENTAR, tamano, color, 0.5);
        }
        Estado::Cerrando => {
            iconos::mostrar_girando(ui, Icono::CONECTANDO, tamano, color, 1.0);
        }
        Estado::Activo => {
            iconos::mostrar(ui, Icono::ACTIVO, tamano, color);
        }
        Estado::Degradado => {
            iconos::mostrar(ui, Icono::DEGRADADO, tamano, color);
        }
        Estado::Fallido => {
            iconos::mostrar(ui, Icono::FALLIDO, tamano, color);
        }
        Estado::Definido => {
            iconos::mostrar(ui, Icono::DEFINIDO, tamano, color);
        }
    }
}

/// Etiqueta de estado: chip con el color del estado.
pub fn etiqueta_estado(ui: &mut egui::Ui, tema: &Tema, estado: Estado) -> egui::Response {
    chip(
        ui,
        tema,
        None,
        estado.etiqueta(),
        tema.color_estado(estado),
        tema.fondo_estado(estado),
    )
}

/// Botón principal: relleno con el acento.
///
/// Solo puede haber uno por vista. El acento es una señal, no un relleno: si
/// todo destaca, no destaca nada.
pub fn boton_principal(
    ui: &mut egui::Ui,
    tema: &Tema,
    icono: Icono,
    texto: &str,
    activo: bool,
) -> egui::Response {
    // El fondo del hover se decide antes de dibujar, con el identificador que
    // tendrá el botón: así el color de realce sale del token y no del cálculo
    // automático de egui, que aclararía el acento sin control.
    let id_previsto = ui.next_auto_id();
    let encima = ui
        .ctx()
        .read_response(id_previsto)
        .is_some_and(|r| r.hovered());

    let (fondo, tinta) = if !activo {
        // Deshabilitado en neutro, no en acento desvaído: un acento a media
        // opacidad deja el texto por debajo del contraste legible y encima
        // sigue pareciendo que invita a pulsarlo.
        (
            if tema.paleta.oscuro {
                tema.paleta.elevado
            } else {
                tema.paleta.hover
            },
            tema.paleta.texto_tenue,
        )
    } else if encima {
        (tema.paleta.acento_hover, tema.paleta.texto_sobre_acento)
    } else {
        (tema.paleta.acento, tema.paleta.texto_sobre_acento)
    };

    let boton = egui::Button::image_and_text(
        iconos::imagen(icono, iconos::NORMAL, tinta),
        egui::RichText::new(texto)
            .size(tema.tipografia.cuerpo)
            .color(tinta),
    )
    .fill(fondo)
    .stroke(Stroke::NONE)
    .corner_radius(CornerRadius::same(tema.radios.medio));

    ui.add_enabled(activo, boton)
}

/// Botón secundario: contorno, sin relleno.
pub fn boton_secundario(
    ui: &mut egui::Ui,
    tema: &Tema,
    icono: Icono,
    texto: &str,
    activo: bool,
) -> egui::Response {
    let tinta = if activo {
        tema.paleta.texto
    } else {
        tema.paleta.texto_tenue
    };
    let boton = egui::Button::image_and_text(
        iconos::imagen(icono, iconos::NORMAL, tinta),
        egui::RichText::new(texto)
            .size(tema.tipografia.cuerpo)
            .color(tinta),
    )
    .fill(Color32::TRANSPARENT)
    .stroke(Stroke::new(1.0, tema.paleta.borde))
    .corner_radius(CornerRadius::same(tema.radios.medio));

    ui.add_enabled(activo, boton)
}

/// Botón destructivo: contorno de error, relleno solo al pasar por encima.
pub fn boton_destructivo(
    ui: &mut egui::Ui,
    tema: &Tema,
    icono: Icono,
    texto: &str,
) -> egui::Response {
    let boton = egui::Button::image_and_text(
        iconos::imagen(icono, iconos::NORMAL, tema.paleta.error),
        egui::RichText::new(texto)
            .size(tema.tipografia.cuerpo)
            .color(tema.paleta.error),
    )
    .fill(Color32::TRANSPARENT)
    .stroke(Stroke::new(1.0, tema.paleta.error.gamma_multiply(0.5)))
    .corner_radius(CornerRadius::same(tema.radios.medio));
    ui.add(boton)
}

/// Ancho de la barra de acento de una tarjeta.
const ANCHO_ACENTO: f32 = 3.0;

/// Barra de acento vertical dentro del borde izquierdo de una tarjeta.
///
/// Es el estilo «chip»: la identidad va en una barra de 3 px, no en un fondo
/// saturado que dominaría la fila entera.
///
/// Va en forma de píldora y separada de los bordes en lugar de pegada a la
/// esquina. Una barra a ras del borde con el radio de la tarjeta se ve
/// deformada —el radio no cabe en 3 px de ancho— y asoma por fuera del trazo.
/// Esto es de lo que separa «pulido» de «amateur» aunque nadie sepa nombrarlo.
pub fn barra_acento(ui: &egui::Ui, rect: egui::Rect, color: Color32, separacion: f32) {
    let barra = egui::Rect::from_min_size(
        egui::pos2(rect.left() + separacion, rect.top() + separacion),
        Vec2::new(ANCHO_ACENTO, (rect.height() - separacion * 2.0).max(0.0)),
    );
    ui.painter().rect_filled(
        barra,
        // Radio = mitad del ancho: extremos perfectamente redondeados.
        CornerRadius::same((ANCHO_ACENTO / 2.0).ceil() as u8),
        color,
    );
}

/// Divisor apenas perceptible.
///
/// El espacio separa mejor que las líneas; cuando hace falta una línea, que sea
/// la mínima que cumple su función.
pub fn divisor(ui: &mut egui::Ui, tema: &Tema) {
    let ancho = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ancho, 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, tema.paleta.divisor),
    );
}

/// Campo de texto con etiqueta encima y ayuda debajo.
pub fn campo(
    ui: &mut egui::Ui,
    tema: &Tema,
    etiqueta: &str,
    valor: &mut String,
    ayuda: Option<&str>,
) -> egui::Response {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = tema.escala.xs;
        ui.label(super::tema::secundario(tema, etiqueta));
        let respuesta = ui.add(
            egui::TextEdit::singleline(valor)
                .desired_width(f32::INFINITY)
                .margin(tema.margen_simetrico(tema.escala.s, tema.escala.xs + 2.0))
                .background_color(if tema.paleta.oscuro {
                    tema.paleta.fondo
                } else {
                    tema.paleta.superficie
                }),
        );
        if let Some(ayuda) = ayuda {
            ui.label(super::tema::tenue(tema, ayuda));
        }
        respuesta
    })
    .inner
}

/// Caja de aviso con icono, para errores y avisos dentro de una pantalla.
pub fn caja_aviso(ui: &mut egui::Ui, tema: &Tema, grave: bool, texto: &str) {
    let (color, fondo, icono) = if grave {
        (tema.paleta.error, tema.paleta.error_suave, Icono::ALERTA)
    } else {
        (tema.paleta.aviso, tema.paleta.aviso_suave, Icono::INFO)
    };

    egui::Frame::new()
        .fill(fondo)
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(tema.radios.medio))
        .inner_margin(tema.margen(tema.escala.m))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = tema.escala.s;
                iconos::mostrar(ui, icono, iconos::NORMAL, color);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(texto)
                            .size(tema.tipografia.pequeno)
                            .color(tema.paleta.texto),
                    )
                    .wrap(),
                );
            });
        });
}

/// Casilla de verificación con el acento del tema.
pub fn casilla(ui: &mut egui::Ui, tema: &Tema, marcada: &mut bool) -> egui::Response {
    let lado = 16.0;
    let (rect, respuesta) = ui.allocate_exact_size(Vec2::splat(lado), Sense::click());
    if respuesta.clicked() {
        *marcada = !*marcada;
    }
    if ui.is_rect_visible(rect) {
        let radio = CornerRadius::same(tema.radios.pequeno);
        let encima = respuesta.hovered();
        if *marcada {
            ui.painter().rect_filled(rect, radio, tema.paleta.acento);
            let marca = iconos::imagen(Icono::ACEPTAR, lado - 3.0, tema.paleta.texto_sobre_acento);
            marca.paint_at(ui, rect.shrink(1.5));
        } else {
            ui.painter().rect_filled(
                rect,
                radio,
                if encima {
                    tema.paleta.hover
                } else {
                    Color32::TRANSPARENT
                },
            );
            ui.painter().rect_stroke(
                rect,
                radio,
                Stroke::new(
                    1.0,
                    if encima {
                        tema.paleta.borde_fuerte
                    } else {
                        tema.paleta.borde
                    },
                ),
                StrokeKind::Inside,
            );
        }
    }
    respuesta
}

/// Cabecera de sección: título en mayúsculas y tenue.
pub fn cabecera_seccion(ui: &mut egui::Ui, tema: &Tema, texto: &str) {
    ui.add_space(tema.escala.s);
    ui.label(
        egui::RichText::new(texto.to_uppercase())
            .size(tema.tipografia.micro)
            .color(tema.paleta.texto_tenue),
    );
    ui.add_space(tema.escala.xs);
}

/// Fila de una lista de propiedades: etiqueta a la izquierda, valor a la derecha.
pub fn propiedad(ui: &mut egui::Ui, tema: &Tema, etiqueta: &str, valor: &str) {
    ui.horizontal(|ui| {
        ui.label(super::tema::tenue(tema, etiqueta));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(super::tema::secundario(tema, valor));
        });
    });
}

/// Texto de una duración en lenguaje natural y corto.
pub fn duracion_legible(duracion: std::time::Duration) -> String {
    let segundos = duracion.as_secs();
    match segundos {
        0..=59 => format!("{segundos} s"),
        60..=3599 => format!("{} min", segundos / 60),
        3600..=86_399 => format!("{} h", segundos / 3600),
        _ => format!("{} d", segundos / 86_400),
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use std::time::Duration;

    #[test]
    fn la_duracion_se_escribe_con_la_unidad_util() {
        assert_eq!(duracion_legible(Duration::from_secs(0)), "0 s");
        assert_eq!(duracion_legible(Duration::from_secs(59)), "59 s");
        assert_eq!(duracion_legible(Duration::from_secs(60)), "1 min");
        assert_eq!(duracion_legible(Duration::from_secs(3599)), "59 min");
        assert_eq!(duracion_legible(Duration::from_secs(3600)), "1 h");
        assert_eq!(duracion_legible(Duration::from_secs(86_400)), "1 d");
    }
}
