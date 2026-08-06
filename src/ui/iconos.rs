//! Iconos: subconjunto de [Lucide](https://lucide.dev) embebido en el binario.
//!
//! Los SVG llevan `stroke="#ffffff"` en vez de `currentColor` (véase
//! `scripts/extraer-iconos.sh`): resvg, el rasterizador que usa egui, no
//! resuelve `currentColor`, y un icono blanco se tinta a cualquier color
//! multiplicando, que es como funciona `Image::tint`. El color siempre sale de
//! un token del tema, nunca de un hex escrito aquí.
//!
//! Van embebidos con `include_bytes!` para que el binario de las *releases* sea
//! autocontenido: descargar un fichero y ejecutarlo, sin instalar nada.
//!
//! Lucide se distribuye bajo licencia ISC. Véase `assets/iconos/LICENCIA`.

use eframe::egui::{self, Color32, Vec2};

/// Un icono del catálogo. El nombre coincide con el fichero de Lucide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icono(&'static str);

impl Icono {
    pub fn nombre(&self) -> &'static str {
        self.0
    }
}

/// Declara las constantes y la tabla de bytes a la vez.
///
/// Que salgan de la misma macro impide que se separen: no puede existir una
/// constante sin su fichero, porque `include_bytes!` fallaría al compilar.
macro_rules! catalogo {
    ($($constante:ident => $archivo:literal),* $(,)?) => {
        // El catálogo es una paleta: existe entero para que la interfaz pueda
        // echar mano de cualquiera de ellos sin volver a tocar el script de
        // extracción. Que hoy no se usen todos no es un descuido.
        #[allow(dead_code)]
        impl Icono {
            $(pub const $constante: Icono = Icono($archivo);)*
        }

        fn fuente_de(nombre: &str) -> Option<egui::ImageSource<'static>> {
            match nombre {
                $($archivo => Some(egui::ImageSource::Bytes {
                    uri: std::borrow::Cow::Borrowed(
                        concat!("bytes://iconos/", $archivo, ".svg")
                    ),
                    bytes: egui::load::Bytes::Static(
                        include_bytes!(concat!("../../assets/iconos/", $archivo, ".svg"))
                    ),
                }),)*
                _ => None,
            }
        }

        /// Todos los iconos del catálogo. Lo usan las pruebas de integridad.
        #[allow(dead_code)]
        pub fn todos() -> &'static [Icono] {
            &[$(Icono($archivo)),*]
        }
    };
}

catalogo! {
    // Identidad y navegación
    TUNEL          => "waypoints",
    SERVIDOR       => "server",
    RED            => "network",
    CABLE          => "cable",
    GLOBO          => "globe",
    TERMINAL       => "terminal",
    PANEL          => "panel-left",
    LISTA          => "list",

    // Acciones
    ARRANCAR       => "play",
    PARAR          => "square",
    ANADIR         => "plus",
    EDITAR         => "pencil",
    BORRAR         => "trash-2",
    COPIAR         => "copy",
    GUARDAR        => "save",
    BUSCAR         => "search",
    CERRAR         => "x",
    ACEPTAR        => "check",
    REINTENTAR     => "refresh-cw",
    ENCENDIDO      => "power",
    IMPORTAR       => "download",
    AJUSTES        => "settings",
    CONTROLES      => "sliders-horizontal",
    DESPLEGAR      => "chevron-right",
    ABIERTO        => "chevron-down",
    PLEGAR         => "chevron-up",
    MAS            => "ellipsis",

    // Estados de §4
    DEFINIDO       => "circle-dot",
    CONECTANDO     => "loader-circle",
    ACTIVO         => "circle-check",
    DEGRADADO      => "triangle-alert",
    FALLIDO        => "circle-x",
    ALERTA         => "circle-alert",

    // Seguridad
    VERIFICADO     => "shield-check",
    SIN_VERIFICAR  => "shield-alert",
    CLAVE          => "key-round",
    BLOQUEADO      => "lock",
    DESBLOQUEADO   => "lock-open",
    LLAVE_FISICA   => "usb",
    FICHERO_CLAVE  => "file-key",

    // Información
    INFO           => "info",
    RELOJ          => "clock",
    HISTORIAL      => "history",
    ACTIVIDAD      => "activity",
    MEDIDOR        => "gauge",
    GRAFICA        => "chart-line",
    RAYO           => "zap",
    ENLACE         => "link-2",
    INTERCAMBIO    => "arrow-right-left",

    // Tema
    CLARO          => "sun",
    OSCURO         => "moon",
    AUTOMATICO     => "monitor-cog",

    // Topología del detalle: este equipo y el servicio de destino
    PORTATIL       => "laptop",
    BASE_DATOS     => "database",
}

/// Tamaños de icono. Tres, a juego con la escala tipográfica.
pub const PEQUENO: f32 = 14.0;
pub const NORMAL: f32 = 16.0;
pub const GRANDE: f32 = 20.0;
pub const ENORME: f32 = 32.0;

/// Construye la imagen de un icono, tintada y al tamaño exacto.
///
/// `fit_to_exact_size` hace que el SVG se rasterice a la resolución final en
/// píxeles físicos, así que el trazo sale nítido también en pantallas HiDPI.
pub fn imagen(icono: Icono, tamano: f32, color: Color32) -> egui::Image<'static> {
    let fuente = fuente_de(icono.nombre()).unwrap_or_else(|| {
        // No puede pasar: la macro garantiza que toda constante tiene fichero.
        // Si pasara, es mejor un hueco que un pánico en mitad del dibujado.
        tracing::error!(icono = icono.nombre(), "icono sin fichero embebido");
        fuente_de("circle-alert").expect("el icono de alerta debe existir")
    });
    egui::Image::new(fuente)
        .fit_to_exact_size(Vec2::splat(tamano))
        .tint(color)
}

/// Dibuja un icono como parte del contenido de un `Ui`.
pub fn mostrar(ui: &mut egui::Ui, icono: Icono, tamano: f32, color: Color32) -> egui::Response {
    ui.add(imagen(icono, tamano, color))
}

/// Dibuja un icono girando sobre sí mismo.
///
/// Se usa en `Conectando` y `Reintentando`: un estado en tránsito que no se
/// mueve parece un estado atascado.
pub fn mostrar_girando(
    ui: &mut egui::Ui,
    icono: Icono,
    tamano: f32,
    color: Color32,
    vueltas_por_segundo: f32,
) -> egui::Response {
    let (rect, respuesta) = ui.allocate_exact_size(Vec2::splat(tamano), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let tiempo = ui.input(|entrada| entrada.time) as f32;
        let angulo = tiempo * vueltas_por_segundo * std::f32::consts::TAU;
        imagen(icono, tamano, color)
            .rotate(angulo, Vec2::splat(0.5))
            .paint_at(ui, rect);
        // Sin esto la animación se congelaría hasta el siguiente evento.
        ui.ctx().request_repaint();
    }
    respuesta
}

/// Botón de solo icono, cuadrado y sin fondo hasta que se pasa por encima.
pub fn boton(ui: &mut egui::Ui, icono: Icono, color: Color32, ayuda: &str) -> egui::Response {
    let lado = ui.spacing().interact_size.y.max(NORMAL + 10.0);
    let respuesta = ui
        .add_sized(
            Vec2::splat(lado),
            egui::Button::image(imagen(icono, NORMAL, color))
                .frame(true)
                .fill(Color32::TRANSPARENT)
                .stroke(egui::Stroke::NONE),
        )
        .on_hover_text(ayuda);
    respuesta
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn todo_icono_del_catalogo_tiene_su_svg_embebido() {
        for icono in todos() {
            assert!(
                fuente_de(icono.nombre()).is_some(),
                "el icono «{}» no tiene fichero",
                icono.nombre()
            );
        }
    }

    #[test]
    fn los_svg_estan_preparados_para_tintarse() {
        // Si un icono llegara con `currentColor`, resvg lo pintaría negro y el
        // tintado no haría nada: sería invisible en tema oscuro.
        let fuente = fuente_de(Icono::ACTIVO.nombre()).unwrap();
        let bytes = match fuente {
            egui::ImageSource::Bytes { bytes, .. } => bytes,
            _ => panic!("se esperaban bytes embebidos"),
        };
        let texto = String::from_utf8_lossy(&bytes);
        assert!(
            !texto.contains("currentColor"),
            "el SVG conserva currentColor: pásalo por scripts/extraer-iconos.sh"
        );
        assert!(texto.contains("#ffffff"), "el trazo no es blanco");
    }

    #[test]
    fn un_nombre_que_no_existe_no_devuelve_fuente() {
        assert!(fuente_de("este-icono-no-existe").is_none());
    }

    #[test]
    fn el_catalogo_no_tiene_duplicados() {
        let mut nombres: Vec<&str> = todos().iter().map(|i| i.nombre()).collect();
        let total = nombres.len();
        nombres.sort_unstable();
        nombres.dedup();
        assert_eq!(nombres.len(), total, "hay iconos repetidos en el catálogo");
    }
}
