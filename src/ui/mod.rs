//! Interfaz gráfica (fase 3).
//!
//! Se apoya entera en el motor de la fase 1: aquí no hay lógica de túneles, solo
//! presentación y despacho de órdenes al supervisor. El modo inmediato de egui
//! encaja con §2.4 —el estado real vive en los sockets de control, la interfaz
//! solo lo pinta— y evita la clase de error en la que la vista y la realidad se
//! separan sin que nadie se entere.
//!
//! Estructura de la ventana:
//!
//! ```text
//! ┌─ barra superior ──────────────── buscar ── acciones ── tema ─┐
//! ├──────────────────────────────────┬──────────────────────────┤
//! │  lista de túneles                │  detalle del seleccionado │
//! ├──────────────────────────────────┴──────────────────────────┤
//! └─ barra de estado: recuento · OpenSSH · fichero de config ────┘
//! ```

mod editor;
mod iconos;
mod lista;
mod modales;
mod tareas;
mod tema;
mod widgets;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui;

use yonder::askpass::{self, PeticionPendiente};
use yonder::modelo::Host;
use yonder::prefs::Preferencias;
use yonder::ssh::{self, Entorno};
use yonder::state::machine::Estado;
use yonder::state::supervisor::{Aviso, Instantanea, Orden, Supervisor};
use yonder::state::Motor;

use iconos::Icono;
use tema::Tema;

/// Cuánto se queda un aviso en pantalla antes de desvanecerse.
const DURACION_AVISO: Duration = Duration::from_secs(8);

/// Recorrido del tamaño de la interfaz.
///
/// El techo del 200 % no es arbitrario: es lo que pide la pauta 1.4.4 de las
/// WCAG, que exige poder ampliar el texto hasta el doble sin que la maquetación
/// se rompa ni se pierda contenido.
pub const ESCALA_MINIMA: f32 = 0.8;
pub const ESCALA_MAXIMA: f32 = 2.0;
/// Salto de cada pulsación, en tanto por uno.
const ESCALON: f32 = 0.05;

/// El siguiente escalón de tamaño en la dirección indicada.
///
/// Se redondea al múltiplo del escalón antes de saltar: si el valor viene de
/// haberlo tecleado o arrastrado —113 %, por ejemplo—, la primera pulsación lo
/// deja en un número redondo en vez de arrastrar el desajuste para siempre.
pub fn escalon_siguiente(actual: f32, direccion: i32) -> f32 {
    let pasos = (actual / ESCALON).round() + direccion as f32;
    (pasos * ESCALON).clamp(ESCALA_MINIMA, ESCALA_MAXIMA)
}

/// Arranca la interfaz gráfica.
pub fn ejecutar() -> anyhow::Result<()> {
    let preferencias = Preferencias::cargar();

    // El askpass tiene que estar en marcha **antes** que el motor: en cuanto se
    // levante el primer túnel, `ssh` puede necesitarlo (§5.1).
    let servidor_askpass = match askpass::Servidor::arrancar() {
        Ok(servidor) => Some(servidor),
        Err(e) => {
            tracing::warn!("sin servidor de askpass: las contraseñas no se podrán pedir ({e})");
            None
        }
    };

    let entorno = match (&servidor_askpass, askpass::localizar_binario()) {
        (Some(_), Some(binario)) => {
            tracing::info!(binario = %binario.display(), "askpass gráfico activo");
            Entorno::grafico(binario)
        }
        (Some(_), None) => {
            tracing::warn!(
                "no se encontró «yonder-askpass» junto al ejecutable ni en el PATH; \
                 los hosts con contraseña no funcionarán desde la ventana"
            );
            Entorno::terminal()
        }
        _ => Entorno::terminal(),
    };

    let mut motor = Motor::nuevo(entorno)?;
    motor.opciones = preferencias.opciones_maestro();
    motor.intervalo_salud = preferencias.intervalo_salud();
    let automaticos = motor.de_autoarranque();
    let version_ssh = ssh::version_openssh().unwrap_or_else(|_| "OpenSSH (desconocido)".into());

    let supervisor = Supervisor::arrancar(motor, preferencias.politica_reintento());
    if preferencias.autoarranque_al_abrir && !automaticos.is_empty() {
        supervisor.enviar(Orden::LevantarVarios(automaticos));
    }

    let opciones = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Yonder")
            .with_app_id("yonder")
            .with_inner_size([1080.0, 680.0])
            .with_min_inner_size([720.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "yonder",
        opciones,
        Box::new(move |contexto| {
            // Sin esto los SVG no se rasterizan y solo se verían huecos.
            egui_extras::install_image_loaders(&contexto.egui_ctx);

            let repintar = contexto.egui_ctx.clone();
            supervisor.al_cambiar(move || repintar.request_repaint());
            if let Some(servidor) = &servidor_askpass {
                let repintar = contexto.egui_ctx.clone();
                servidor.al_llegar(move || repintar.request_repaint());
            }

            Ok(Box::new(Aplicacion::nueva(
                supervisor,
                servidor_askpass,
                preferencias,
                version_ssh,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("no se pudo abrir la ventana: {e}"))
}

/// Qué modal está abierto. Solo puede haber uno.
pub enum Modal {
    Ninguno,
    /// Alta o edición de un host.
    Editor(editor::EstadoEditor),
    /// `ssh` pide una contraseña (§5.1).
    Askpass {
        peticion: Option<PeticionPendiente>,
        respuesta: String,
    },
    /// Verificación de clave de host (§5.2).
    ClaveHost(modales::EstadoClaveHost),
    /// Intercambio de claves (§5.5).
    CopiarClave(modales::EstadoCopiarClave),
    /// Importación de hosts externos (§3.1).
    Importar,
    Ajustes,
    /// Confirmación destructiva.
    Confirmacion {
        titulo: String,
        texto: String,
        etiqueta_aceptar: String,
        orden: Option<Orden>,
    },
    /// Error con sugerencia.
    Error {
        titulo: String,
        texto: String,
    },
}

impl Modal {
    pub fn abierto(&self) -> bool {
        !matches!(self, Modal::Ninguno)
    }
}

pub struct Aplicacion {
    supervisor: Supervisor,
    askpass: Option<askpass::Servidor>,
    preferencias: Preferencias,
    tema: Tema,
    version_ssh: String,

    instantanea: Instantanea,
    /// Casillas marcadas para las acciones en bloque.
    seleccion: HashSet<String>,
    /// Túnel cuyo detalle se muestra en el panel derecho.
    detalle: Option<String>,
    filtro: String,
    solo_problemas: bool,
    modal: Modal,
    aviso: Option<(Aviso, Instant)>,
    /// La ventana ya avisó de que falta el `Include` en `~/.ssh/config`.
    include_avisado: bool,
}

impl Aplicacion {
    fn nueva(
        supervisor: Supervisor,
        askpass: Option<askpass::Servidor>,
        preferencias: Preferencias,
        version_ssh: String,
    ) -> Aplicacion {
        let tema = Tema::nuevo(&preferencias, true);
        Aplicacion {
            supervisor,
            askpass,
            preferencias,
            tema,
            version_ssh,
            instantanea: Instantanea::default(),
            seleccion: HashSet::new(),
            detalle: None,
            filtro: String::new(),
            solo_problemas: false,
            modal: Modal::Ninguno,
            aviso: None,
            include_avisado: false,
        }
    }

    /// Envía una orden al supervisor.
    pub fn ordenar(&self, orden: Orden) {
        self.supervisor.enviar(orden);
    }

    pub fn tema(&self) -> &Tema {
        &self.tema
    }

    pub fn instantanea(&self) -> &Instantanea {
        &self.instantanea
    }

    /// Host al que pertenece un alias.
    pub fn host(&self, alias: &str) -> Option<&Host> {
        self.instantanea.hosts.iter().find(|h| h.alias == alias)
    }

    fn refrescar(&mut self, contexto: &egui::Context) {
        self.instantanea = self.supervisor.instantanea();

        if let Some(nuevo) = self.supervisor.tomar_aviso() {
            self.aviso = Some((nuevo, Instant::now()));
        }
        if let Some((_, desde)) = &self.aviso {
            if desde.elapsed() > DURACION_AVISO {
                self.aviso = None;
            }
        }

        // Una petición de contraseña tiene prioridad sobre lo que hubiera:
        // al otro lado hay un `ssh` esperando con un temporizador.
        if let Some(servidor) = &self.askpass {
            if !matches!(self.modal, Modal::Askpass { .. }) {
                if let Some(peticion) = servidor.siguiente() {
                    self.modal = Modal::Askpass {
                        peticion: Some(peticion),
                        respuesta: String::new(),
                    };
                }
            }
        }

        // El tema del escritorio puede cambiar mientras la ventana está abierta.
        let oscuro = contexto.style().visuals.dark_mode;
        self.tema = Tema::nuevo(&self.preferencias, oscuro);
        self.tema.aplicar(contexto);

        // La selección no debe conservar túneles que ya no existen.
        let vigentes: HashSet<String> = self.instantanea.tuneles.iter().map(|t| t.id()).collect();
        self.seleccion.retain(|id| vigentes.contains(id));
        if let Some(id) = &self.detalle {
            if !vigentes.contains(id) {
                self.detalle = None;
            }
        }
    }

    pub fn mostrar_error(&mut self, titulo: impl Into<String>, texto: impl Into<String>) {
        self.modal = Modal::Error {
            titulo: titulo.into(),
            texto: texto.into(),
        };
    }

    pub fn cerrar_modal(&mut self) {
        self.modal = Modal::Ninguno;
    }

    /// Túneles que pasan el filtro de búsqueda y el de problemas.
    fn visibles(&self) -> Vec<yonder::modelo::Tunel> {
        let filtro = self.filtro.trim().to_lowercase();
        self.instantanea
            .tuneles
            .iter()
            .filter(|tunel| {
                if self.solo_problemas {
                    let estado = self
                        .instantanea
                        .estado(&tunel.id())
                        .map(|e| e.estado)
                        .unwrap_or(Estado::Definido);
                    if !estado.problematico() {
                        return false;
                    }
                }
                if filtro.is_empty() {
                    return true;
                }
                let host = self.host(&tunel.alias);
                let campos = [
                    tunel.alias.to_lowercase(),
                    tunel.reenvio.descripcion().to_lowercase(),
                    host.map(|h| h.destino_completo().to_lowercase())
                        .unwrap_or_default(),
                    host.and_then(|h| h.nota.clone())
                        .unwrap_or_default()
                        .to_lowercase(),
                ];
                campos.iter().any(|campo| campo.contains(&filtro))
            })
            .cloned()
            .collect()
    }

    // --- Barras -----------------------------------------------------------

    fn barra_superior(&mut self, contexto: &egui::Context) {
        let tema = self.tema;
        let marco = egui::Frame::new()
            .fill(tema.paleta.superficie)
            .inner_margin(tema.margen_simetrico(tema.escala.l, tema.escala.m))
            .stroke(egui::Stroke::NONE);

        egui::TopBottomPanel::top("barra_superior")
            .frame(marco)
            .show(contexto, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = tema.escala.s;

                    iconos::mostrar(ui, Icono::TUNEL, iconos::GRANDE, tema.paleta.acento);
                    ui.label(
                        egui::RichText::new("Yonder")
                            .size(tema.tipografia.titulo)
                            .color(tema.paleta.texto),
                    );

                    ui.add_space(tema.escala.l);
                    self.campo_busqueda(ui);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.acciones_globales(ui);
                    });
                });
            });

        // Franja de aviso justo bajo la barra: no tapa nada y se ve seguro.
        if let Some((aviso, _)) = self.aviso.clone() {
            let marco_aviso = egui::Frame::new()
                .fill(if aviso.grave {
                    tema.paleta.error_suave
                } else {
                    tema.paleta.info_suave
                })
                .inner_margin(tema.margen_simetrico(tema.escala.l, tema.escala.s));

            egui::TopBottomPanel::top("franja_aviso")
                .frame(marco_aviso)
                .show(contexto, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = tema.escala.s;
                        let color = if aviso.grave {
                            tema.paleta.error
                        } else {
                            tema.paleta.info
                        };
                        iconos::mostrar(
                            ui,
                            if aviso.grave {
                                Icono::ALERTA
                            } else {
                                Icono::INFO
                            },
                            iconos::NORMAL,
                            color,
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&aviso.texto)
                                    .size(tema.tipografia.pequeno)
                                    .color(tema.paleta.texto),
                            )
                            .wrap(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if iconos::boton(
                                ui,
                                Icono::CERRAR,
                                tema.paleta.texto_tenue,
                                "Descartar",
                            )
                            .clicked()
                            {
                                self.aviso = None;
                            }
                        });
                    });
                });
        }
    }

    fn campo_busqueda(&mut self, ui: &mut egui::Ui) {
        let tema = self.tema;
        egui::Frame::new()
            .fill(if tema.paleta.oscuro {
                tema.paleta.fondo
            } else {
                tema.paleta.hover
            })
            .corner_radius(egui::CornerRadius::same(tema.radios.medio))
            .inner_margin(tema.margen_simetrico(tema.escala.s, tema.escala.xs))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = tema.escala.xs;
                    iconos::mostrar(ui, Icono::BUSCAR, iconos::PEQUENO, tema.paleta.texto_tenue);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filtro)
                            .desired_width(200.0)
                            .frame(false)
                            .hint_text(
                                egui::RichText::new("Buscar túnel, host o nota")
                                    .size(tema.tipografia.pequeno)
                                    .color(tema.paleta.texto_tenue),
                            ),
                    );
                    if !self.filtro.is_empty()
                        && iconos::boton(ui, Icono::CERRAR, tema.paleta.texto_tenue, "Limpiar")
                            .clicked()
                    {
                        self.filtro.clear();
                    }
                });
            });
    }

    fn acciones_globales(&mut self, ui: &mut egui::Ui) {
        let tema = self.tema;
        ui.spacing_mut().item_spacing.x = tema.escala.xs;

        // Tema: un solo botón que rota entre automático, claro y oscuro.
        let (icono_tema, ayuda) = match self.preferencias.tema {
            yonder::prefs::Tema::Auto => (Icono::AUTOMATICO, "Tema: automático"),
            yonder::prefs::Tema::Claro => (Icono::CLARO, "Tema: claro"),
            yonder::prefs::Tema::Oscuro => (Icono::OSCURO, "Tema: oscuro"),
        };
        if iconos::boton(ui, icono_tema, tema.paleta.texto_secundario, ayuda).clicked() {
            self.preferencias.tema = self.preferencias.tema.siguiente();
            self.guardar_preferencias();
        }

        if iconos::boton(ui, Icono::AJUSTES, tema.paleta.texto_secundario, "Ajustes").clicked() {
            self.modal = Modal::Ajustes;
        }

        if iconos::boton(
            ui,
            Icono::IMPORTAR,
            tema.paleta.texto_secundario,
            "Importar túneles ya definidos en ~/.ssh/config",
        )
        .clicked()
        {
            self.modal = Modal::Importar;
        }

        if iconos::boton(
            ui,
            Icono::REINTENTAR,
            tema.paleta.texto_secundario,
            "Recargar la configuración del disco",
        )
        .clicked()
        {
            self.ordenar(Orden::Recargar);
        }

        ui.add_space(tema.escala.s);

        // Acciones en bloque sobre la selección.
        let seleccionados: Vec<String> = self.seleccion.iter().cloned().collect();
        let hay_seleccion = !seleccionados.is_empty();

        if widgets::boton_secundario(ui, &tema, Icono::PARAR, "Parar", hay_seleccion).clicked() {
            self.ordenar(Orden::BajarVarios(seleccionados.clone()));
            self.seleccion.clear();
        }
        if widgets::boton_principal(
            ui,
            &tema,
            Icono::ARRANCAR,
            &if hay_seleccion {
                format!("Iniciar {}", seleccionados.len())
            } else {
                "Iniciar".to_string()
            },
            hay_seleccion,
        )
        .clicked()
        {
            self.ordenar(Orden::LevantarVarios(seleccionados));
            self.seleccion.clear();
        }

        ui.add_space(tema.escala.s);

        if widgets::boton_secundario(ui, &tema, Icono::ANADIR, "Nuevo túnel", true).clicked() {
            self.modal = Modal::Editor(editor::EstadoEditor::nuevo());
        }
    }

    fn barra_estado(&mut self, contexto: &egui::Context) {
        let tema = self.tema;
        let marco = egui::Frame::new()
            .fill(tema.paleta.superficie)
            .inner_margin(tema.margen_simetrico(tema.escala.l, tema.escala.s));

        egui::TopBottomPanel::bottom("barra_estado")
            .frame(marco)
            .show(contexto, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = tema.escala.m;

                    let recuento = |estado: Estado| self.instantanea.cuenta(estado);
                    self.contador(ui, Estado::Activo, recuento(Estado::Activo));
                    self.contador(ui, Estado::Degradado, recuento(Estado::Degradado));
                    self.contador(ui, Estado::Reintentando, recuento(Estado::Reintentando));
                    self.contador(ui, Estado::Fallido, recuento(Estado::Fallido));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(tema::tenue(
                            &tema,
                            yonder::rutas::config_tuneles()
                                .map(|r| yonder::rutas::abreviar(&r))
                                .unwrap_or_default(),
                        ));
                        ui.label(tema::tenue(&tema, "·"));
                        ui.label(tema::tenue(&tema, &self.version_ssh));
                        ui.label(tema::tenue(&tema, "·"));
                        ui.label(tema::tenue(
                            &tema,
                            format!("{} túneles", self.instantanea.tuneles.len()),
                        ));
                    });
                });
            });
    }

    fn contador(&self, ui: &mut egui::Ui, estado: Estado, cantidad: usize) {
        if cantidad == 0 && !matches!(estado, Estado::Activo) {
            return;
        }
        let tema = self.tema;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.xs;
            widgets::indicador_estado(ui, &tema, estado, iconos::PEQUENO);
            ui.label(
                egui::RichText::new(format!("{cantidad} {}", estado.etiqueta().to_lowercase()))
                    .size(tema.tipografia.pequeno)
                    .color(if cantidad > 0 && estado.problematico() {
                        tema.color_estado(estado)
                    } else {
                        tema.paleta.texto_secundario
                    }),
            );
        });
    }

    /// El detalle, en una ventana flotante con cierre explícito.
    ///
    /// No es un panel lateral —le quitaba ancho a todas las filas para enseñar
    /// los datos de una sola— ni un desplegable en línea, que empujaba los
    /// túneles de abajo cada vez que se consultaba uno. Flota sobre la lista,
    /// se puede mover y se queda hasta que se cierra: mientras se compara un
    /// dato con otro, no desaparece por pasar el ratón a otro sitio.
    fn ventana_detalle(&mut self, contexto: &egui::Context) {
        let Some(id) = self.detalle.clone() else {
            return;
        };
        let tema = self.tema;
        let titulo = self
            .instantanea
            .tuneles
            .iter()
            .find(|t| t.id() == id)
            .map(|t| t.alias.clone())
            .unwrap_or_else(|| id.clone());

        let mut abierta = true;
        egui::Window::new(titulo)
            .id(egui::Id::new("ventana_detalle"))
            .open(&mut abierta)
            .frame(tema.marco_modal())
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .default_pos(contexto.screen_rect().center() - egui::vec2(260.0, 240.0))
            .show(contexto, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(520.0)
                    .show(ui, |ui| {
                        lista::panel_detalle(self, ui, &id);
                    });
            });
        if !abierta {
            self.detalle = None;
        }
    }

    // --- Preferencias ------------------------------------------------------

    pub fn preferencias_mut(&mut self) -> &mut Preferencias {
        &mut self.preferencias
    }

    pub fn guardar_preferencias(&mut self) {
        if let Err(e) = self.preferencias.guardar() {
            tracing::warn!("no se pudieron guardar las preferencias: {e}");
        }
    }
}

impl eframe::App for Aplicacion {
    fn update(&mut self, contexto: &egui::Context, _marco: &mut eframe::Frame) {
        // Atajos de tamaño, los mismos que usa cualquier navegador. Se
        // consumen para que no lleguen a otro control.
        let ajuste = contexto.input_mut(|e| {
            let ctrl = egui::Modifiers::COMMAND;
            if e.consume_key(ctrl, egui::Key::Plus) || e.consume_key(ctrl, egui::Key::Equals) {
                1
            } else if e.consume_key(ctrl, egui::Key::Minus) {
                -1
            } else if e.consume_key(ctrl, egui::Key::Num0) {
                0
            } else {
                i32::MIN
            }
        });
        if ajuste != i32::MIN {
            self.preferencias.escala_interfaz = if ajuste == 0 {
                1.0
            } else {
                escalon_siguiente(self.preferencias.escala_interfaz, ajuste)
            };
            self.guardar_preferencias();
        }

        // Escala de la interfaz. El zoom de egui toca letra y espaciado a la
        // vez, así que subirlo no descuadra las cajas.
        let escala = self
            .preferencias
            .escala_interfaz
            .clamp(ESCALA_MINIMA, ESCALA_MAXIMA);
        if (contexto.zoom_factor() - escala).abs() > f32::EPSILON {
            contexto.set_zoom_factor(escala);
        }

        self.refrescar(contexto);
        self.barra_superior(contexto);
        self.barra_estado(contexto);
        self.ventana_detalle(contexto);

        let tema = self.tema;
        egui::CentralPanel::default()
            .frame(tema.marco_panel())
            .show(contexto, |ui| {
                let visibles = self.visibles();
                lista::mostrar(self, ui, &visibles);
            });

        modales::mostrar(self, contexto);

        // Un túnel en tránsito o esperando reintento necesita repintado
        // periódico para que el contador y el giro avancen.
        if self
            .instantanea
            .estados
            .iter()
            .any(|e| e.estado.en_transito())
        {
            contexto.request_repaint_after(Duration::from_millis(100));
        } else {
            contexto.request_repaint_after(Duration::from_secs(1));
        }
    }
}

/// Comprueba si falta la línea `Include` y devuelve el aviso, una sola vez.
///
/// Sin ella se rompe el criterio de aceptación 7 de §12: `ssh <alias>` desde
/// una terminal no vería los túneles, y todo el diseño de §3.1 dejaría de tener
/// sentido.
pub fn aviso_include(aplicacion: &mut Aplicacion) -> Option<String> {
    if aplicacion.include_avisado {
        return None;
    }
    aplicacion.include_avisado = true;
    match yonder::config::estado_include() {
        Ok(yonder::config::EstadoInclude::Presente) => None,
        Ok(_) => Some(
            "~/.ssh/config no incluye el fichero de túneles. Sin esa línea, «ssh <alias>» \
             desde una terminal no los verá."
                .to_string(),
        ),
        Err(_) => None,
    }
}
