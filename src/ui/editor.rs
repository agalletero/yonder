//! Alta y edición de un host con sus reenvíos.
//!
//! El formulario valida **antes** de escribir nada: puerto ocupado, puerto
//! privilegiado sin capacidad, alias con comodines, dos reenvíos en el mismo
//! puerto. Descubrir eso al pulsar «levantar» y no al guardar convierte un aviso
//! claro en un fallo confuso tres pantallas más tarde (§5.3, §5.4).

use eframe::egui;

use yonder::modelo::{Extremo, Host, Reenvio, Salud, TipoReenvio};
use yonder::net::probe;
use yonder::state::supervisor::Orden;

use super::iconos::{self, Icono};
use super::tema::{self, Tema};
use super::widgets;
use super::{Aplicacion, Modal};

/// Una fila de reenvío en el formulario, con los campos como texto.
///
/// Se guardan como texto y no como números para que el usuario pueda borrar el
/// contenido de un campo sin que salte a cero mientras escribe.
#[derive(Debug, Clone, Default)]
pub struct FilaReenvio {
    pub tipo: TipoReenvio,
    pub direccion_escucha: String,
    pub puerto_escucha: String,
    pub host_destino: String,
    pub puerto_destino: String,
    /// Cómo comprobar que el túnel transporta de verdad.
    pub salud: Salud,
    /// Ruta de la comprobación HTTP. Se guarda aparte para que no se pierda al
    /// cambiar de tipo de sonda y volver.
    pub ruta_http: String,
}

impl FilaReenvio {
    fn de(reenvio: &Reenvio) -> FilaReenvio {
        FilaReenvio {
            tipo: reenvio.tipo,
            direccion_escucha: reenvio.escucha.direccion.clone().unwrap_or_default(),
            puerto_escucha: reenvio.escucha.puerto.to_string(),
            host_destino: reenvio
                .destino
                .as_ref()
                .and_then(|d| d.direccion.clone())
                .unwrap_or_default(),
            puerto_destino: reenvio
                .destino
                .as_ref()
                .map(|d| d.puerto.to_string())
                .unwrap_or_default(),
            ruta_http: match &reenvio.salud {
                Salud::Http { ruta } => ruta.clone(),
                _ => "/".to_string(),
            },
            salud: reenvio.salud.clone(),
        }
    }

    fn nueva() -> FilaReenvio {
        FilaReenvio {
            tipo: TipoReenvio::Local,
            direccion_escucha: String::new(),
            puerto_escucha: String::new(),
            host_destino: "localhost".to_string(),
            puerto_destino: String::new(),
            salud: Salud::Escucha,
            ruta_http: "/".to_string(),
        }
    }

    /// Salud efectiva, con la ruta del campo aparte si es una sonda HTTP.
    fn salud_efectiva(&self) -> Salud {
        match &self.salud {
            Salud::Http { .. } => Salud::Http {
                ruta: if self.ruta_http.trim().is_empty() {
                    "/".to_string()
                } else {
                    self.ruta_http.trim().to_string()
                },
            },
            otra => otra.clone(),
        }
    }

    /// Convierte la fila en un reenvío, o explica por qué no se puede.
    fn a_reenvio(&self) -> Result<Reenvio, String> {
        let puerto_escucha: u16 = self
            .puerto_escucha
            .trim()
            .parse()
            .map_err(|_| format!("«{}» no es un puerto válido", self.puerto_escucha.trim()))?;
        if puerto_escucha == 0 {
            return Err("el puerto 0 no es válido".to_string());
        }

        let escucha = Extremo {
            direccion: Some(self.direccion_escucha.trim().to_string()).filter(|d| !d.is_empty()),
            puerto: puerto_escucha,
        };

        if self.tipo == TipoReenvio::Dinamico {
            return Ok(Reenvio {
                tipo: self.tipo,
                escucha,
                destino: None,
                // Un proxy SOCKS no habla HTTP ni saluda: sondearlo con esas
                // comprobaciones daría un falso zombi permanente.
                salud: Salud::Escucha,
            });
        }

        let host = self.host_destino.trim();
        if host.is_empty() {
            return Err("falta el host de destino".to_string());
        }
        let puerto_destino: u16 = self
            .puerto_destino
            .trim()
            .parse()
            .map_err(|_| format!("«{}» no es un puerto válido", self.puerto_destino.trim()))?;
        if puerto_destino == 0 {
            return Err("el puerto de destino no puede ser 0".to_string());
        }

        Ok(Reenvio {
            tipo: self.tipo,
            escucha,
            destino: Some(Extremo::nuevo(host, puerto_destino)),
            salud: if self.tipo == TipoReenvio::Local {
                self.salud_efectiva()
            } else {
                // Un reenvío remoto escucha en la otra punta: desde aquí no hay
                // nada que sondear.
                Salud::Escucha
            },
        })
    }
}

/// Estado del formulario.
pub struct EstadoEditor {
    /// Alias original; `None` si es un host nuevo.
    pub original: Option<String>,
    pub alias: String,
    pub hostname: String,
    pub usuario: String,
    pub puerto: String,
    pub saltos: String,
    pub identidad: String,
    pub nota: String,
    pub reenvios: Vec<FilaReenvio>,
    /// Errores de validación, uno por línea.
    pub errores: Vec<String>,
    /// Avisos que no impiden guardar.
    pub avisos: Vec<String>,
}

impl EstadoEditor {
    pub fn nuevo() -> EstadoEditor {
        EstadoEditor {
            original: None,
            alias: String::new(),
            hostname: String::new(),
            usuario: String::new(),
            puerto: String::new(),
            saltos: String::new(),
            identidad: String::new(),
            nota: String::new(),
            reenvios: vec![FilaReenvio::nueva()],
            errores: Vec::new(),
            avisos: Vec::new(),
        }
    }

    pub fn de(host: &Host) -> EstadoEditor {
        EstadoEditor {
            original: Some(host.alias.clone()),
            alias: host.alias.clone(),
            hostname: host.hostname.clone().unwrap_or_default(),
            usuario: host.usuario.clone().unwrap_or_default(),
            puerto: host.puerto.map(|p| p.to_string()).unwrap_or_default(),
            saltos: host.saltos.join(", "),
            identidad: host.identidades.first().cloned().unwrap_or_default(),
            nota: host.nota.clone().unwrap_or_default(),
            reenvios: if host.reenvios.is_empty() {
                vec![FilaReenvio::nueva()]
            } else {
                host.reenvios.iter().map(FilaReenvio::de).collect()
            },
            errores: Vec::new(),
            avisos: Vec::new(),
        }
    }

    pub fn titulo(&self) -> String {
        match &self.original {
            Some(alias) => format!("Editar «{alias}»"),
            None => "Nuevo túnel".to_string(),
        }
    }

    /// Construye el host a partir del formulario, acumulando errores y avisos.
    fn construir(&mut self) -> Option<Host> {
        self.errores.clear();
        self.avisos.clear();

        let mut host = Host::nuevo(self.alias.trim());
        host.hostname = Some(self.hostname.trim().to_string()).filter(|v| !v.is_empty());
        host.usuario = Some(self.usuario.trim().to_string()).filter(|v| !v.is_empty());
        host.nota = Some(self.nota.trim().to_string()).filter(|v| !v.is_empty());

        if !self.puerto.trim().is_empty() {
            match self.puerto.trim().parse::<u16>() {
                Ok(puerto) if puerto > 0 => host.puerto = Some(puerto),
                _ => self.errores.push(format!(
                    "«{}» no es un puerto SSH válido",
                    self.puerto.trim()
                )),
            }
        }

        host.saltos = self
            .saltos
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if !self.identidad.trim().is_empty() {
            host.identidades.push(self.identidad.trim().to_string());
        }

        for (indice, fila) in self.reenvios.iter().enumerate() {
            match fila.a_reenvio() {
                Ok(reenvio) => host.reenvios.push(reenvio),
                Err(motivo) => self
                    .errores
                    .push(format!("Reenvío {}: {motivo}", indice + 1)),
            }
        }

        if host.reenvios.is_empty() && self.errores.is_empty() {
            self.errores
                .push("Hay que definir al menos un reenvío".to_string());
        }

        if let Err(e) = host.validar() {
            self.errores.push(e.to_string());
        }

        // Avisos que no impiden guardar pero que conviene ver ahora y no cuando
        // falle la conexión.
        for reenvio in &host.reenvios {
            if !reenvio.tipo.escucha_en_local() {
                continue;
            }
            let puerto = reenvio.escucha.puerto;
            if puerto < 1024 && !probe::puede_abrir_puertos_privilegiados() {
                self.avisos.push(format!(
                    "El puerto {puerto} es privilegiado y este proceso no tiene \
                     CAP_NET_BIND_SERVICE. Concédela con:\n    {}",
                    probe::orden_para_conceder_capacidad()
                ));
            } else if let Err(e) =
                probe::comprobar_puerto_libre(reenvio.escucha.direccion_efectiva(), puerto)
            {
                self.avisos.push(e.to_string());
            }
            if reenvio.escucha.expuesto() {
                self.avisos.push(format!(
                    "El reenvío del puerto {puerto} escucha en todas las interfaces: \
                     cualquiera de tu red podrá usarlo."
                ));
            }
        }

        if self.errores.is_empty() {
            Some(host)
        } else {
            None
        }
    }
}

/// Dibuja el editor. Devuelve `true` si hay que cerrarlo.
pub fn mostrar(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    let Modal::Editor(estado) = &mut aplicacion.modal else {
        return true;
    };

    ui.set_width(560.0);

    ui.horizontal(|ui| {
        iconos::mostrar(ui, Icono::TUNEL, iconos::GRANDE, tema.paleta.acento);
        ui.label(
            egui::RichText::new(estado.titulo())
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });
    ui.add_space(tema.escala.s);
    ui.label(tema::tenue(
        &tema,
        "Se guarda en ~/.ssh/config.d/yonder.conf. Los comentarios y las \
         directivas que la aplicación no gestiona se conservan tal cual.",
    ));

    ui.add_space(tema.escala.m);
    widgets::divisor(ui, &tema);
    ui.add_space(tema.escala.m);

    let mut guardar = false;
    let mut cerrar = false;

    egui::ScrollArea::vertical()
        .max_height(440.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = tema.escala.m;

            widgets::campo(
                ui,
                &tema,
                "Alias",
                &mut estado.alias,
                Some("El nombre con el que lo llamarás: «ssh <alias>» desde la terminal."),
            );

            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = tema.escala.m;
                let ancho = (ui.available_width() - tema.escala.m * 2.0) / 3.0;
                ui.allocate_ui(egui::vec2(ancho, 0.0), |ui| {
                    widgets::campo(ui, &tema, "Host de destino", &mut estado.hostname, None);
                });
                ui.allocate_ui(egui::vec2(ancho, 0.0), |ui| {
                    widgets::campo(ui, &tema, "Usuario", &mut estado.usuario, None);
                });
                ui.allocate_ui(egui::vec2(ancho, 0.0), |ui| {
                    widgets::campo(ui, &tema, "Puerto SSH", &mut estado.puerto, None);
                });
            });

            widgets::campo(
                ui,
                &tema,
                "Máquinas de salto",
                &mut estado.saltos,
                Some("ProxyJump. Varias separadas por comas; OpenSSH encadena los saltos."),
            );

            widgets::campo(
                ui,
                &tema,
                "Clave privada",
                &mut estado.identidad,
                Some(
                    "IdentityFile. Déjalo vacío para que decida ssh-agent. \
                     Una clave «_sk» pedirá tocar la llave física.",
                ),
            );

            widgets::campo(ui, &tema, "Nota", &mut estado.nota, None);

            ui.add_space(tema.escala.s);
            widgets::cabecera_seccion(ui, &tema, "Reenvíos");

            let mut borrar: Option<usize> = None;
            let total = estado.reenvios.len();
            for (indice, fila) in estado.reenvios.iter_mut().enumerate() {
                fila_reenvio(ui, &tema, indice, fila, total > 1, &mut borrar);
            }
            if let Some(indice) = borrar {
                estado.reenvios.remove(indice);
            }

            ui.add_space(tema.escala.xs);
            if widgets::boton_secundario(ui, &tema, Icono::ANADIR, "Añadir reenvío", true).clicked()
            {
                estado.reenvios.push(FilaReenvio::nueva());
            }

            if !estado.errores.is_empty() {
                ui.add_space(tema.escala.s);
                widgets::caja_aviso(ui, &tema, true, &estado.errores.join("\n"));
            }
            for aviso in &estado.avisos {
                ui.add_space(tema.escala.xs);
                widgets::caja_aviso(ui, &tema, false, aviso);
            }
        });

    ui.add_space(tema.escala.m);
    widgets::divisor(ui, &tema);
    ui.add_space(tema.escala.m);

    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            if widgets::boton_principal(ui, &tema, Icono::GUARDAR, "Guardar", true).clicked() {
                guardar = true;
            }
            if widgets::boton_secundario(ui, &tema, Icono::CERRAR, "Cancelar", true).clicked() {
                cerrar = true;
            }
        });
    });

    if guardar {
        let original = estado.original.clone();
        if let Some(host) = estado.construir() {
            let orden = match original {
                Some(antiguo) if antiguo != host.alias => Orden::RenombrarHost {
                    antiguo,
                    nuevo: Box::new(host),
                },
                _ => Orden::GuardarHost(Box::new(host)),
            };
            aplicacion.ordenar(orden);
            return true;
        }
        // Con errores el modal se queda abierto para poder corregirlos.
    }

    cerrar
}

fn fila_reenvio(
    ui: &mut egui::Ui,
    tema: &Tema,
    indice: usize,
    fila: &mut FilaReenvio,
    se_puede_borrar: bool,
    borrar: &mut Option<usize>,
) {
    egui::Frame::new()
        .fill(if tema.paleta.oscuro {
            tema.paleta.fondo
        } else {
            tema.paleta.hover
        })
        .corner_radius(egui::CornerRadius::same(tema.radios.medio))
        .inner_margin(tema.margen(tema.escala.s))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = tema.escala.s;

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = tema.escala.s;
                ui.label(tema::tenue(tema, format!("#{}", indice + 1)));

                egui::ComboBox::from_id_salt(format!("tipo_reenvio_{indice}"))
                    .selected_text(
                        egui::RichText::new(etiqueta_tipo(fila.tipo))
                            .size(tema.tipografia.pequeno)
                            .color(tema.paleta.texto),
                    )
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for tipo in [
                            TipoReenvio::Local,
                            TipoReenvio::Remoto,
                            TipoReenvio::Dinamico,
                        ] {
                            ui.selectable_value(&mut fila.tipo, tipo, etiqueta_tipo(tipo));
                        }
                    });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if se_puede_borrar
                        && iconos::boton(ui, Icono::BORRAR, tema.paleta.texto_tenue, "Quitar")
                            .clicked()
                    {
                        *borrar = Some(indice);
                    }
                });
            });

            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = tema.escala.s;
                let columnas = if fila.tipo == TipoReenvio::Dinamico {
                    2.0
                } else {
                    4.0
                };
                let ancho = (ui.available_width() - tema.escala.s * (columnas - 1.0)) / columnas;

                ui.allocate_ui(egui::vec2(ancho, 0.0), |ui| {
                    widgets::campo(
                        ui,
                        tema,
                        "Escucha en",
                        &mut fila.direccion_escucha,
                        Some("vacío = localhost"),
                    );
                });
                ui.allocate_ui(egui::vec2(ancho, 0.0), |ui| {
                    widgets::campo(ui, tema, "Puerto local", &mut fila.puerto_escucha, None);
                });

                if fila.tipo != TipoReenvio::Dinamico {
                    ui.allocate_ui(egui::vec2(ancho, 0.0), |ui| {
                        widgets::campo(ui, tema, "Host remoto", &mut fila.host_destino, None);
                    });
                    ui.allocate_ui(egui::vec2(ancho, 0.0), |ui| {
                        widgets::campo(ui, tema, "Puerto remoto", &mut fila.puerto_destino, None);
                    });
                }
            });

            // Comprobación de salud. Solo para reenvíos locales: un SOCKS no
            // habla HTTP y un reenvío remoto escucha en la otra punta.
            if fila.tipo == TipoReenvio::Local {
                selector_de_salud(ui, tema, indice, fila);
            }

            // Vista previa de las líneas que acabarán en el fichero. Ver el
            // resultado exacto evita la sorpresa de abrir el .conf después.
            match fila.a_reenvio() {
                Ok(reenvio) => {
                    if reenvio.salud != Salud::Escucha {
                        ui.label(tema::mono(
                            tema,
                            format!("# {} {}", Salud::MARCA, reenvio.salud.especificacion()),
                        ));
                    }
                    ui.label(tema::mono(
                        tema,
                        format!("{} {}", reenvio.tipo.directiva(), reenvio.valor_directiva()),
                    ));
                }
                Err(motivo) => {
                    ui.label(
                        egui::RichText::new(motivo)
                            .size(tema.tipografia.micro)
                            .color(tema.paleta.texto_tenue),
                    );
                }
            }
        });
}

/// Selector de la comprobación de salud de un reenvío.
///
/// Es el control que separa «el puerto está abierto» de «el túnel transporta».
/// Sin él la interfaz solo puede saber lo primero, y hay casos —un destino que
/// apunta a donde el servicio remoto no escucha— en los que lo primero es cierto
/// y lo segundo no.
fn selector_de_salud(ui: &mut egui::Ui, tema: &Tema, indice: usize, fila: &mut FilaReenvio) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, Icono::MEDIDOR, iconos::PEQUENO, tema.paleta.texto_tenue);
        ui.label(tema::secundario(tema, "Comprobación"));

        egui::ComboBox::from_id_salt(format!("salud_{indice}"))
            .selected_text(
                egui::RichText::new(fila.salud.etiqueta())
                    .size(tema.tipografia.pequeno)
                    .color(tema.paleta.texto),
            )
            .width(150.0)
            .show_ui(ui, |ui| {
                for opcion in Salud::opciones() {
                    let elegida = fila.salud.misma_clase(&opcion);
                    if ui
                        .selectable_label(elegida, opcion.etiqueta())
                        .on_hover_text(opcion.explicacion())
                        .clicked()
                    {
                        fila.salud = opcion.clone();
                    }
                }
            });

        if matches!(fila.salud, Salud::Http { .. }) {
            ui.add(
                egui::TextEdit::singleline(&mut fila.ruta_http)
                    .desired_width(160.0)
                    .hint_text("/api/health"),
            );
        }
    });

    ui.label(tema::tenue(tema, fila.salud.explicacion()));
    if fila.salud.atraviesa_el_tunel() {
        ui.label(tema::tenue(
            tema,
            "Abre una conexión real por el túnel cada pocos segundos: detecta el \
             túnel zombi, pero se nota al otro lado.",
        ));
    }
}

fn etiqueta_tipo(tipo: TipoReenvio) -> &'static str {
    match tipo {
        TipoReenvio::Local => "Local  ·  puerto de aquí → allí",
        TipoReenvio::Remoto => "Remoto  ·  puerto de allí → aquí",
        TipoReenvio::Dinamico => "SOCKS  ·  proxy dinámico",
    }
}
