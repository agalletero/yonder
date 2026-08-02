//! Modales: contraseñas, clave de host, intercambio de claves, importación,
//! ajustes y confirmaciones.
//!
//! Solo puede haber uno abierto. Los que atienden a `ssh` —contraseña y clave de
//! host— tienen prioridad sobre el resto: al otro lado hay un proceso esperando
//! con un temporizador corriendo.

use eframe::egui;

use yonder::askpass::Tipo;
use yonder::modelo::Host;
use yonder::ssh::{self, hostkey::ClaveHost};
use yonder::state::supervisor::Orden;

use super::iconos::{self, Icono};
use super::tareas::Tarea;
use super::tema::{self, Tema};
use super::widgets;
use super::{editor, Aplicacion, Modal};

/// Despacha el modal que toque.
pub fn mostrar(aplicacion: &mut Aplicacion, contexto: &egui::Context) {
    if !aplicacion.modal.abierto() {
        return;
    }
    let tema = *aplicacion.tema();

    let respuesta = egui::Modal::new(egui::Id::new("modal_principal"))
        .frame(tema.marco_modal())
        .backdrop_color(if tema.paleta.oscuro {
            egui::Color32::from_black_alpha(160)
        } else {
            egui::Color32::from_black_alpha(70)
        })
        .show(contexto, |ui| {
            ui.spacing_mut().item_spacing.y = tema.escala.s;
            match &mut aplicacion.modal {
                Modal::Editor(_) => editor::mostrar(aplicacion, ui),
                Modal::Askpass { .. } => askpass(aplicacion, ui),
                Modal::ClaveHost(_) => clave_host(aplicacion, ui),
                Modal::CopiarClave(_) => copiar_clave(aplicacion, ui),
                Modal::Importar => importar(aplicacion, ui),
                Modal::Ajustes => ajustes(aplicacion, ui),
                Modal::Confirmacion { .. } => confirmacion(aplicacion, ui),
                Modal::Error { .. } => error(aplicacion, ui),
                Modal::Ninguno => true,
            }
        });

    // El fondo cierra el modal, salvo el del askpass: dejar a `ssh` colgado por
    // un clic despistado sería peor que exigir una respuesta explícita.
    let cierre_por_fondo =
        respuesta.should_close() && !matches!(aplicacion.modal, Modal::Askpass { .. });

    if respuesta.inner || cierre_por_fondo {
        // Un askpass que se cierra sin responder debe cancelar de verdad, no
        // dejar al proceso esperando hasta que salte el tiempo máximo.
        if let Modal::Askpass { peticion, .. } = &mut aplicacion.modal {
            if let Some(peticion) = peticion.take() {
                peticion.cancelar();
            }
        }
        aplicacion.cerrar_modal();
    }
}

// --- Askpass (§5.1) --------------------------------------------------------

fn askpass(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    let Modal::Askpass {
        peticion,
        respuesta,
    } = &mut aplicacion.modal
    else {
        return true;
    };
    let Some(pendiente) = peticion.as_ref() else {
        return true;
    };

    let tipo = pendiente.tipo;
    let texto_prompt = pendiente.texto_visible();

    ui.set_width(440.0);

    let (icono, color) = match tipo {
        Tipo::PresenciaFisica => (Icono::LLAVE_FISICA, tema.paleta.acento),
        Tipo::PinLlave => (Icono::LLAVE_FISICA, tema.paleta.acento),
        Tipo::Passphrase => (Icono::CLAVE, tema.paleta.acento),
        Tipo::Confirmacion => (Icono::ALERTA, tema.paleta.aviso),
        _ => (Icono::BLOQUEADO, tema.paleta.acento),
    };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, icono, iconos::GRANDE, color);
        ui.label(
            egui::RichText::new(tipo.titulo())
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });

    ui.add_space(tema.escala.s);
    ui.add(egui::Label::new(tema::secundario(&tema, &texto_prompt)).wrap());
    ui.add_space(tema.escala.m);

    let mut aceptar = false;
    let mut cancelar = false;

    if tipo.sin_entrada() {
        // No hay nada que teclear: solo hay que tocar la llave. La ventana
        // acompaña, no estorba.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            iconos::mostrar_girando(ui, Icono::CONECTANDO, iconos::NORMAL, color, 0.8);
            ui.label(tema::cuerpo(
                &tema,
                "Esperando a que toques la llave física…",
            ));
        });
    } else {
        let campo = ui.add(
            egui::TextEdit::singleline(respuesta)
                .password(tipo.es_secreto())
                .desired_width(f32::INFINITY)
                .margin(tema.margen_simetrico(tema.escala.s, tema.escala.xs + 2.0))
                .hint_text(if tipo == Tipo::Confirmacion {
                    "yes"
                } else {
                    ""
                }),
        );
        // El foco va al campo en cuanto aparece: si hay que ir a buscarlo con
        // el ratón, el modal estorba en vez de ayudar.
        if !campo.has_focus() {
            campo.request_focus();
        }
        if campo.lost_focus() && ui.input(|entrada| entrada.key_pressed(egui::Key::Enter)) {
            aceptar = true;
        }
    }

    ui.add_space(tema.escala.m);
    widgets::caja_aviso(
        ui,
        &tema,
        false,
        "Lo que escribas va directo al proceso ssh. No se guarda en ningún sitio \
         salvo que lo pidas expresamente en los ajustes del host.",
    );

    ui.add_space(tema.escala.m);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            if !tipo.sin_entrada()
                && widgets::boton_principal(ui, &tema, Icono::ACEPTAR, "Enviar", true).clicked()
            {
                aceptar = true;
            }
            if widgets::boton_secundario(ui, &tema, Icono::CERRAR, "Cancelar", true).clicked() {
                cancelar = true;
            }
        });
    });

    if aceptar {
        if let Some(pendiente) = peticion.take() {
            pendiente.responder(respuesta.clone());
        }
        respuesta.clear();
        return true;
    }
    if cancelar {
        if let Some(pendiente) = peticion.take() {
            pendiente.cancelar();
        }
        return true;
    }
    false
}

// --- Clave de host (§5.2) --------------------------------------------------

/// Estado del modal de verificación de clave de host.
pub struct EstadoClaveHost {
    pub alias: String,
    pub host: String,
    pub puerto: u16,
    pub tarea: Option<Tarea<Vec<ClaveHost>>>,
    pub claves: Vec<ClaveHost>,
    pub error: Option<String>,
    pub ya_conocido: bool,
}

impl EstadoClaveHost {
    pub fn nuevo(aplicacion: &Aplicacion, alias: &str) -> EstadoClaveHost {
        let definicion = aplicacion.host(alias);
        let host = definicion
            .map(|h| h.destino().to_string())
            .unwrap_or_else(|| alias.to_string());
        let puerto = definicion.and_then(|h| h.puerto).unwrap_or(22);
        EstadoClaveHost {
            alias: alias.to_string(),
            host,
            puerto,
            tarea: None,
            claves: Vec::new(),
            error: None,
            ya_conocido: false,
        }
    }
}

fn clave_host(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    let contexto = ui.ctx().clone();
    let Modal::ClaveHost(estado) = &mut aplicacion.modal else {
        return true;
    };

    ui.set_width(560.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, Icono::VERIFICADO, iconos::GRANDE, tema.paleta.acento);
        ui.label(
            egui::RichText::new(format!("Verificar la clave de «{}»", estado.alias))
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });
    ui.add_space(tema.escala.s);
    ui.label(tema::secundario(
        &tema,
        format!("{} · puerto {}", estado.host, estado.puerto),
    ));

    // Primer fotograma: se lanza el escaneo.
    if estado.tarea.is_none() && estado.claves.is_empty() && estado.error.is_none() {
        let host = estado.host.clone();
        let puerto = estado.puerto;
        estado.ya_conocido = ssh::hostkey::ya_conocido(&host, puerto).unwrap_or(false);
        estado.tarea = Some(Tarea::lanzar(&contexto, "ssh-keyscan", move || {
            ssh::hostkey::escanear(&host, puerto)
        }));
    }

    if let Some(tarea) = &mut estado.tarea {
        match tarea.resultado() {
            Some(Ok(claves)) => {
                estado.claves = claves;
                estado.tarea = None;
            }
            Some(Err(e)) => {
                estado.error = Some(e.to_string());
                estado.tarea = None;
            }
            None => {}
        }
    }

    ui.add_space(tema.escala.m);

    if let Some(tarea) = estado.tarea.as_ref().filter(|t| t.en_curso()) {
        let descripcion = tarea.descripcion.clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            iconos::mostrar_girando(ui, Icono::CONECTANDO, iconos::NORMAL, tema.paleta.info, 1.0);
            ui.label(tema::cuerpo(&tema, "Escaneando las claves del host…"));
            ui.label(tema::tenue(&tema, format!("({descripcion})")));
        });
        return false;
    }

    if let Some(error) = &estado.error {
        widgets::caja_aviso(ui, &tema, true, error);
        ui.add_space(tema.escala.m);
        return pie_cerrar(ui, &tema);
    }

    if estado.ya_conocido {
        widgets::caja_aviso(
            ui,
            &tema,
            false,
            "Este host ya está en known_hosts. Aceptar de nuevo añadiría una entrada \
             duplicada; solo tiene sentido si sabes que la clave ha cambiado por un \
             motivo legítimo.",
        );
        ui.add_space(tema.escala.m);
    }

    widgets::cabecera_seccion(ui, &tema, "Fingerprints que presenta el host");
    for clave in &estado.claves {
        egui::Frame::new()
            .fill(if tema.paleta.oscuro {
                tema.paleta.fondo
            } else {
                tema.paleta.hover
            })
            .corner_radius(egui::CornerRadius::same(tema.radios.medio))
            .inner_margin(tema.margen(tema.escala.s))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = tema.escala.s;
                    widgets::chip_neutro(ui, &tema, Icono::CLAVE, &clave.algoritmo());
                    ui.label(tema::tenue(&tema, format!("{} bits", clave.bits)));
                });
                ui.add_space(tema.escala.xs);
                ui.label(
                    egui::RichText::new(&clave.fingerprint)
                        .monospace()
                        .size(tema.tipografia.cuerpo)
                        .color(tema.paleta.texto),
                );
            });
        ui.add_space(tema.escala.xs);
    }

    ui.add_space(tema.escala.s);
    widgets::caja_aviso(
        ui,
        &tema,
        true,
        "Compara este fingerprint con el que te conste POR OTRO CANAL antes de aceptarlo. \
         Escanear no verifica nada: solo trae lo que el host dice ser. Si hay alguien en \
         medio, esto es lo que te enseñaría.",
    );

    ui.add_space(tema.escala.m);
    let mut cerrar = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            if widgets::boton_principal(ui, &tema, Icono::ACEPTAR, "Coincide, aceptar", true)
                .clicked()
            {
                match ssh::hostkey::aceptar(&estado.claves) {
                    Ok(()) => cerrar = true,
                    Err(e) => estado.error = Some(e.to_string()),
                }
            }
            if widgets::boton_secundario(ui, &tema, Icono::CERRAR, "No aceptar", true).clicked() {
                cerrar = true;
            }
        });
    });
    cerrar
}

// --- Intercambio de claves (§5.5) ------------------------------------------

pub struct EstadoCopiarClave {
    pub alias: String,
    pub claves: Vec<std::path::PathBuf>,
    pub elegida: usize,
    pub tarea: Option<Tarea<()>>,
    pub resultado: Option<Result<String, String>>,
}

impl EstadoCopiarClave {
    pub fn nuevo(alias: &str) -> EstadoCopiarClave {
        EstadoCopiarClave {
            alias: alias.to_string(),
            claves: ssh::copyid::claves_publicas_disponibles().unwrap_or_default(),
            elegida: 0,
            tarea: None,
            resultado: None,
        }
    }
}

fn copiar_clave(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    let contexto = ui.ctx().clone();
    let entorno = match yonder::askpass::localizar_binario() {
        Some(binario) => ssh::Entorno::grafico(binario),
        None => ssh::Entorno::terminal(),
    };
    let Modal::CopiarClave(estado) = &mut aplicacion.modal else {
        return true;
    };

    ui.set_width(520.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, Icono::CLAVE, iconos::GRANDE, tema.paleta.acento);
        ui.label(
            egui::RichText::new(format!("Instalar tu clave en «{}»", estado.alias))
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });

    ui.add_space(tema.escala.s);
    ui.label(tema::secundario(
        &tema,
        "Se copia solo la clave PÚBLICA con ssh-copy-id. La privada no sale de \
         tu equipo y esta aplicación nunca la lee.",
    ));
    ui.add_space(tema.escala.m);

    if estado.claves.is_empty() {
        widgets::caja_aviso(
            ui,
            &tema,
            true,
            "No hay ninguna clave pública en ~/.ssh. Genera una primero:\n    \
             ssh-keygen -t ed25519",
        );
        ui.add_space(tema.escala.m);
        return pie_cerrar(ui, &tema);
    }

    if let Some(tarea) = &mut estado.tarea {
        match tarea.resultado() {
            Some(Ok(())) => {
                let solo_clave = ssh::copyid::verificar_solo_clave(&estado.alias).unwrap_or(false);
                estado.resultado = Some(Ok(if solo_clave {
                    "Clave instalada. Comprobado: ya se entra solo con clave pública, \
                     así que puedes borrar del llavero cualquier contraseña de este host."
                        .to_string()
                } else {
                    "Clave instalada, pero la entrada sin contraseña todavía no funciona. \
                     Revisa la configuración del servidor."
                        .to_string()
                }));
                estado.tarea = None;
            }
            Some(Err(e)) => {
                estado.resultado = Some(Err(e.to_string()));
                estado.tarea = None;
            }
            None => {}
        }
    }

    if let Some(resultado) = &estado.resultado {
        match resultado {
            Ok(texto) => widgets::caja_aviso(ui, &tema, false, texto),
            Err(texto) => widgets::caja_aviso(ui, &tema, true, texto),
        }
        ui.add_space(tema.escala.m);
        return pie_cerrar(ui, &tema);
    }

    if let Some(tarea) = estado.tarea.as_ref().filter(|t| t.en_curso()) {
        let descripcion = tarea.descripcion.clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            iconos::mostrar_girando(ui, Icono::CONECTANDO, iconos::NORMAL, tema.paleta.info, 1.0);
            ui.label(tema::cuerpo(
                &tema,
                "Instalando la clave… puede pedirte la contraseña una última vez.",
            ));
            ui.label(tema::tenue(&tema, format!("({descripcion})")));
        });
        return false;
    }

    widgets::cabecera_seccion(ui, &tema, "Clave a instalar");
    for (indice, ruta) in estado.claves.iter().enumerate() {
        let elegida = estado.elegida == indice;
        let hardware = ssh::copyid::es_clave_hardware(ruta);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            if ui.radio(elegida, yonder::rutas::abreviar(ruta)).clicked() {
                estado.elegida = indice;
            }
            if hardware {
                widgets::chip(
                    ui,
                    &tema,
                    Some(Icono::LLAVE_FISICA),
                    "FIDO2",
                    tema.paleta.acento,
                    tema.paleta.acento_suave,
                );
            }
        });
    }

    if estado
        .claves
        .get(estado.elegida)
        .map(|r| ssh::copyid::es_clave_hardware(r))
        .unwrap_or(false)
    {
        ui.add_space(tema.escala.s);
        widgets::caja_aviso(
            ui,
            &tema,
            false,
            "Es una clave respaldada por hardware: tendrás que tocar la llave física \
             durante el proceso.",
        );
    }

    ui.add_space(tema.escala.m);
    let mut cerrar = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            if widgets::boton_principal(ui, &tema, Icono::ACEPTAR, "Instalar", true).clicked() {
                if let Some(ruta) = estado.claves.get(estado.elegida).cloned() {
                    let alias = estado.alias.clone();
                    estado.tarea = Some(Tarea::lanzar(&contexto, "ssh-copy-id", move || {
                        ssh::copyid::copiar_clave(&alias, &ruta, &entorno)
                    }));
                }
            }
            if widgets::boton_secundario(ui, &tema, Icono::CERRAR, "Cancelar", true).clicked() {
                cerrar = true;
            }
        });
    });
    cerrar
}

// --- Importación (§3.1) ----------------------------------------------------

fn importar(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    let candidatos: Vec<Host> = aplicacion.instantanea().importables.clone();

    ui.set_width(560.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, Icono::IMPORTAR, iconos::GRANDE, tema.paleta.acento);
        ui.label(
            egui::RichText::new("Importar túneles existentes")
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });

    ui.add_space(tema.escala.s);
    ui.label(tema::secundario(
        &tema,
        "Importar aquí significa mostrarlos en la lista y poder activarlos. \
         NO se mueven de fichero ni se modifican: siguen viviendo donde estaban \
         y se editan a mano.",
    ));

    ui.add_space(tema.escala.m);
    estado_include(aplicacion, ui);

    ui.add_space(tema.escala.m);

    if candidatos.is_empty() {
        ui.label(tema::tenue(
            &tema,
            "No hay ningún host con reenvíos fuera del fichero propio.",
        ));
        ui.add_space(tema.escala.m);
        return pie_cerrar(ui, &tema);
    }

    let mut a_importar: Vec<String> = Vec::new();

    egui::ScrollArea::vertical()
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for host in &candidatos {
                egui::Frame::new()
                    .fill(if tema.paleta.oscuro {
                        tema.paleta.fondo
                    } else {
                        tema.paleta.hover
                    })
                    .corner_radius(egui::CornerRadius::same(tema.radios.medio))
                    .inner_margin(tema.margen(tema.escala.s))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(tema::cuerpo(&tema, &host.alias));
                                ui.label(tema::tenue(&tema, host.destino_completo()));
                                for reenvio in &host.reenvios {
                                    ui.label(tema::mono(&tema, reenvio.descripcion()));
                                }
                                if let yonder::modelo::Origen::Ajeno(ruta) = &host.origen {
                                    ui.label(tema::tenue(&tema, yonder::rutas::abreviar(ruta)));
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::boton_secundario(
                                        ui,
                                        &tema,
                                        Icono::ANADIR,
                                        "Mostrar",
                                        true,
                                    )
                                    .clicked()
                                    {
                                        a_importar.push(host.alias.clone());
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(tema.escala.xs);
            }
        });

    ui.add_space(tema.escala.m);
    let mut cerrar = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            if widgets::boton_principal(
                ui,
                &tema,
                Icono::IMPORTAR,
                &format!("Mostrar los {}", candidatos.len()),
                true,
            )
            .clicked()
            {
                for host in &candidatos {
                    a_importar.push(host.alias.clone());
                }
                cerrar = true;
            }
            if widgets::boton_secundario(ui, &tema, Icono::CERRAR, "Cerrar", true).clicked() {
                cerrar = true;
            }
        });
    });

    for alias in a_importar {
        aplicacion.ordenar(Orden::Importar(alias));
    }
    cerrar
}

/// Estado de la línea `Include` con el botón para añadirla.
fn estado_include(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) {
    let tema = *aplicacion.tema();
    match yonder::config::estado_include() {
        Ok(yonder::config::EstadoInclude::Presente) => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = tema.escala.s;
                iconos::mostrar(ui, Icono::ACTIVO, iconos::NORMAL, tema.paleta.exito);
                ui.label(tema::secundario(
                    &tema,
                    "~/.ssh/config ya incluye el fichero de túneles",
                ));
            });
        }
        Ok(_) => {
            widgets::caja_aviso(
                ui,
                &tema,
                true,
                "Falta la línea Include en ~/.ssh/config. Sin ella, «ssh <alias>» desde una \
                 terminal no verá estos túneles, que es justamente lo que esta herramienta \
                 promete. Se añade como primera línea, con copia de seguridad previa.",
            );
            ui.add_space(tema.escala.s);
            if widgets::boton_principal(ui, &tema, Icono::ACEPTAR, "Añadir el Include", true)
                .clicked()
            {
                match yonder::config::asegurar_include() {
                    Ok(Some(respaldo)) => aplicacion.mostrar_error(
                        "Include añadido",
                        format!(
                            "Se ha añadido la línea a ~/.ssh/config.\nCopia de seguridad: {}",
                            yonder::rutas::abreviar(&respaldo)
                        ),
                    ),
                    Ok(None) => {}
                    Err(e) => {
                        aplicacion.mostrar_error("No se pudo añadir el Include", e.to_string())
                    }
                }
            }
        }
        Err(e) => widgets::caja_aviso(ui, &tema, true, &e.to_string()),
    }
}

// --- Ajustes ---------------------------------------------------------------

fn ajustes(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    ui.set_width(480.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, Icono::AJUSTES, iconos::GRANDE, tema.paleta.acento);
        ui.label(
            egui::RichText::new("Ajustes")
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });

    ui.add_space(tema.escala.m);
    let mut cambiado = false;

    widgets::cabecera_seccion(ui, &tema, "Apariencia");
    ui.horizontal(|ui| {
        ui.label(tema::cuerpo(&tema, "Tema"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let actual = aplicacion.preferencias_mut().tema;
            for opcion in [
                yonder::prefs::Tema::Oscuro,
                yonder::prefs::Tema::Claro,
                yonder::prefs::Tema::Auto,
            ] {
                if ui
                    .selectable_label(actual == opcion, opcion.etiqueta())
                    .clicked()
                {
                    aplicacion.preferencias_mut().tema = opcion;
                    cambiado = true;
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(tema::cuerpo(&tema, "Densidad"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let actual = aplicacion.preferencias_mut().densidad;
            for opcion in [
                yonder::prefs::Densidad::Comoda,
                yonder::prefs::Densidad::Compacta,
            ] {
                if ui
                    .selectable_label(actual == opcion, opcion.etiqueta())
                    .clicked()
                {
                    aplicacion.preferencias_mut().densidad = opcion;
                    cambiado = true;
                }
            }
        });
    });

    widgets::cabecera_seccion(ui, &tema, "Conexión");
    ui.label(tema::tenue(
        &tema,
        "Estos valores se escriben en el bloque Host, así que también los usa \
         «ssh <alias>» desde la terminal.",
    ));
    ui.add_space(tema.escala.xs);
    cambiado |= deslizador(
        ui,
        &tema,
        "Latido (ServerAliveInterval)",
        &mut aplicacion.preferencias_mut().intervalo_latido,
        5..=120,
        "s",
    );
    cambiado |= deslizador(
        ui,
        &tema,
        "Latidos perdidos antes de darla por caída",
        &mut aplicacion.preferencias_mut().latidos_perdidos,
        1..=10,
        "",
    );
    cambiado |= deslizador(
        ui,
        &tema,
        "Tiempo máximo de conexión",
        &mut aplicacion.preferencias_mut().espera_conexion,
        3..=120,
        "s",
    );
    cambiado |= deslizador(
        ui,
        &tema,
        "Reintentos antes de darlo por fallido (0 = sin límite)",
        &mut aplicacion.preferencias_mut().maximo_reintentos,
        0..=50,
        "",
    );

    widgets::cabecera_seccion(ui, &tema, "Comprobación de salud");
    ui.label(tema::tenue(
        &tema,
        "Las sondas «banner» y «HTTP» abren conexiones reales por el túnel: \
         detectan el túnel zombi, pero se notan en el servicio del otro lado.",
    ));
    ui.add_space(tema.escala.xs);
    cambiado |= deslizador(
        ui,
        &tema,
        "Cada cuánto se sondea",
        &mut aplicacion.preferencias_mut().intervalo_salud,
        5..=600,
        "s",
    );

    widgets::cabecera_seccion(ui, &tema, "Comportamiento");
    let mut autoarranque = aplicacion.preferencias_mut().autoarranque_al_abrir;
    ui.horizontal(|ui| {
        if widgets::casilla(ui, &tema, &mut autoarranque).clicked() {
            aplicacion.preferencias_mut().autoarranque_al_abrir = autoarranque;
            cambiado = true;
        }
        ui.label(tema::cuerpo(
            &tema,
            "Levantar los túneles marcados al abrir la ventana",
        ));
    });

    widgets::cabecera_seccion(ui, &tema, "Rutas");
    for (uso, ruta) in [
        ("Túneles", yonder::rutas::config_tuneles()),
        ("Preferencias", yonder::rutas::preferencias()),
        ("Estado", yonder::rutas::base_de_datos()),
        ("Sockets", yonder::rutas::ejecucion()),
        ("Registro", yonder::rutas::registro()),
    ] {
        if let Ok(ruta) = ruta {
            widgets::propiedad(ui, &tema, uso, &yonder::rutas::abreviar(&ruta));
        }
    }

    if cambiado {
        aplicacion.guardar_preferencias();
    }

    ui.add_space(tema.escala.m);
    pie_cerrar(ui, &tema)
}

fn deslizador(
    ui: &mut egui::Ui,
    tema: &Tema,
    etiqueta: &str,
    valor: &mut u32,
    rango: std::ops::RangeInclusive<u32>,
    unidad: &str,
) -> bool {
    let mut cambiado = false;
    ui.horizontal(|ui| {
        ui.label(tema::secundario(tema, etiqueta));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let respuesta = ui.add(egui::DragValue::new(valor).range(rango).speed(0.2).suffix(
                if unidad.is_empty() {
                    String::new()
                } else {
                    format!(" {unidad}")
                },
            ));
            cambiado = respuesta.changed();
        });
    });
    cambiado
}

// --- Confirmación y error --------------------------------------------------

fn confirmacion(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    let Modal::Confirmacion {
        titulo,
        texto,
        etiqueta_aceptar,
        orden,
    } = &mut aplicacion.modal
    else {
        return true;
    };

    let titulo = titulo.clone();
    let texto = texto.clone();
    let etiqueta = etiqueta_aceptar.clone();

    ui.set_width(440.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, Icono::ALERTA, iconos::GRANDE, tema.paleta.aviso);
        ui.label(
            egui::RichText::new(&titulo)
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });
    ui.add_space(tema.escala.s);
    ui.add(egui::Label::new(tema::secundario(&tema, &texto)).wrap());
    ui.add_space(tema.escala.l);

    let mut confirmado = false;
    let mut cerrar = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            if widgets::boton_destructivo(ui, &tema, Icono::ACEPTAR, &etiqueta).clicked() {
                confirmado = true;
            }
            if widgets::boton_secundario(ui, &tema, Icono::CERRAR, "Cancelar", true).clicked() {
                cerrar = true;
            }
        });
    });

    if confirmado {
        if let Some(orden) = orden.take() {
            aplicacion.ordenar(orden);
        }
        return true;
    }
    cerrar
}

fn error(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) -> bool {
    let tema = *aplicacion.tema();
    let Modal::Error { titulo, texto } = &aplicacion.modal else {
        return true;
    };
    let titulo = titulo.clone();
    let texto = texto.clone();

    ui.set_width(480.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;
        iconos::mostrar(ui, Icono::ALERTA, iconos::GRANDE, tema.paleta.error);
        ui.label(
            egui::RichText::new(&titulo)
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
    });
    ui.add_space(tema.escala.m);
    bloque_codigo(ui, &tema, &texto);
    ui.add_space(tema.escala.m);
    pie_cerrar(ui, &tema)
}

/// Pie con un único botón de cierre.
fn pie_cerrar(ui: &mut egui::Ui, tema: &Tema) -> bool {
    let mut cerrar = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::boton_secundario(ui, tema, Icono::CERRAR, "Cerrar", true).clicked() {
                cerrar = true;
            }
        });
    });
    cerrar
}

/// Bloque monoespaciado seleccionable, para órdenes y mensajes de `ssh`.
pub fn bloque_codigo(ui: &mut egui::Ui, tema: &Tema, texto: &str) {
    egui::Frame::new()
        .fill(if tema.paleta.oscuro {
            tema.paleta.fondo
        } else {
            tema.paleta.hover
        })
        .stroke(egui::Stroke::new(1.0, tema.paleta.borde))
        .corner_radius(egui::CornerRadius::same(tema.radios.medio))
        .inner_margin(tema.margen(tema.escala.s))
        .show(ui, |ui| {
            // Seleccionable: una orden que hay que copiar a mano letra a letra
            // es una orden que no se va a ejecutar.
            let mut copia = texto.to_string();
            ui.add(
                egui::TextEdit::multiline(&mut copia)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .frame(false)
                    .interactive(true),
            );
        });
}
