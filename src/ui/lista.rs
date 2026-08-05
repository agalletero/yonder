//! Lista de túneles y panel de detalle.
//!
//! La lista es una tabla, no una pila de tarjetas: filas pegadas, sin borde ni
//! esquinas propias, con el fondo alternando entre dos tonos vecinos. Con la
//! fila ancha, ese contraste mínimo es lo que permite seguirla con la vista de
//! un extremo al otro sin cambiar de renglón por el camino, y no gasta los ocho
//! o diez píxeles por fila que costaba el margen entre tarjetas.
//!
//! El estado va en el icono y en la barra de acento lateral, nunca en el fondo:
//! con quince túneles en pantalla, un fondo de color por estado sería ilegible
//! y además chocaría con la alternancia.

use eframe::egui::{self, Sense};

use yonder::modelo::{Origen, Tunel};
use yonder::state::machine::{Estado, EstadoTunel};
use yonder::state::supervisor::Orden;

use super::iconos::{self, Icono};
use super::tema::{self, Tema};
use super::widgets;
use super::{editor, modales, Aplicacion, GrupoHost, Modal};

/// Lo que el usuario pide desde una fila. Se acumula y se aplica al final del
/// recorrido para no pelearse con el préstamo de `Aplicacion`.
enum Accion {
    Alternar(String),
    VerDetalle(String),
    Levantar(String),
    Bajar(String),
    Reintentar(String),
    Editar(String),
    PedirEliminar(String),
    VerificarClave(String),
    CopiarClave(String),
}

pub fn mostrar(aplicacion: &mut Aplicacion, ui: &mut egui::Ui, grupos: &[GrupoHost]) {
    let tema = *aplicacion.tema();

    if let Some(texto) = super::aviso_include(aplicacion) {
        widgets::caja_aviso(ui, &tema, false, &texto);
        ui.add_space(tema.escala.m);
    }

    if aplicacion.instantanea().tuneles.is_empty() {
        pantalla_vacia(aplicacion, ui);
        return;
    }

    let visibles: usize = grupos.iter().map(|g| g.tuneles.len()).sum();
    cabecera_lista(aplicacion, ui, visibles);

    if grupos.is_empty() {
        ui.add_space(tema.escala.xl);
        ui.vertical_centered(|ui| {
            iconos::mostrar(ui, Icono::BUSCAR, iconos::ENORME, tema.paleta.texto_tenue);
            ui.add_space(tema.escala.s);
            ui.label(tema::secundario(
                &tema,
                "Ningún túnel coincide con el filtro",
            ));
        });
        return;
    }

    let mut acciones: Vec<Accion> = Vec::new();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut impar = false;
            for grupo in grupos {
                // Un host con un solo reenvío no lleva cabecera: un grupo de un
                // elemento es ruido, y la fila puede decirlo todo ella sola.
                if grupo.tuneles.len() > 1 {
                    cabecera_grupo(aplicacion, ui, grupo, &mut acciones);
                }
                let solo = grupo.tuneles.len() == 1;
                for (tunel, estado) in &grupo.tuneles {
                    fila(
                        aplicacion,
                        ui,
                        tunel,
                        estado,
                        grupo,
                        solo,
                        impar,
                        &mut acciones,
                    );
                    impar = !impar;
                }
                ui.add_space(tema.escala.s);
            }
            ui.add_space(tema.escala.l);
        });

    aplicar(aplicacion, acciones);
}

/// Cabecera de un host: el alias una vez, no una por reenvío.
///
/// Aquí suben el alias, el destino y los chips que antes se repetían en cada
/// fila. Es lo que devuelve a la fila su sitio para decir algo propio.
fn cabecera_grupo(
    aplicacion: &Aplicacion,
    ui: &mut egui::Ui,
    grupo: &GrupoHost,
    _acciones: &mut [Accion],
) {
    let tema = *aplicacion.tema();
    let maestro = grupo.estado_maestro();
    let activos = grupo.activos();

    egui::Frame::new()
        .inner_margin(tema.margen_simetrico(tema.escala.s, tema.escala.xs))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = tema.escala.s;
                ui.label(
                    egui::RichText::new(&grupo.alias)
                        .size(tema.tipografia.titulo)
                        .color(tema.paleta.texto),
                );
                if let Some(host) = &grupo.host {
                    ui.label(tema::mono(&tema, host.destino_completo()));
                    chips_del_host(ui, &tema, Some(host));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // El recuento es lo que se mira de reojo: cuántos de los
                    // que hay deberían estar arriba y lo están.
                    let color = if activos > 0 {
                        tema.paleta.exito
                    } else {
                        tema.paleta.texto_tenue
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{activos} de {} activos",
                            grupo.tuneles.len()
                        ))
                        .size(tema.tipografia.micro)
                        .color(color),
                    );
                    ui.label(tema::tenue(&tema, maestro.etiqueta()));
                });
            });
        });
}

/// Cabecera con el selector de todos y el filtro de problemas.
fn cabecera_lista(aplicacion: &mut Aplicacion, ui: &mut egui::Ui, visibles: usize) {
    let tema = *aplicacion.tema();
    let total = aplicacion.instantanea().tuneles.len();
    let con_problemas = aplicacion
        .instantanea()
        .estados
        .iter()
        .filter(|e| e.estado.problematico())
        .count();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tema.escala.s;

        let todos_marcados = visibles > 0 && aplicacion.seleccion.len() >= visibles;
        let mut marcado = todos_marcados;
        if widgets::casilla(ui, &tema, &mut marcado).clicked() {
            if marcado {
                let ids: Vec<String> = aplicacion
                    .instantanea()
                    .tuneles
                    .iter()
                    .map(|t| t.id())
                    .collect();
                aplicacion.seleccion.extend(ids);
            } else {
                aplicacion.seleccion.clear();
            }
        }

        ui.label(tema::secundario(
            &tema,
            if visibles == total {
                format!("{total} túneles")
            } else {
                format!("{visibles} de {total} túneles")
            },
        ));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if con_problemas > 0 {
                let activo = aplicacion.solo_problemas;
                let color = if activo {
                    tema.paleta.aviso
                } else {
                    tema.paleta.texto_secundario
                };
                let fondo = if activo {
                    tema.paleta.aviso_suave
                } else if tema.paleta.oscuro {
                    tema.paleta.elevado
                } else {
                    tema.paleta.hover
                };
                let etiqueta = format!("{con_problemas} con problemas");
                if widgets::chip(ui, &tema, Some(Icono::DEGRADADO), &etiqueta, color, fondo)
                    .interact(Sense::click())
                    .on_hover_text("Mostrar solo los túneles con problemas")
                    .clicked()
                {
                    aplicacion.solo_problemas = !activo;
                }
            } else if aplicacion.solo_problemas {
                aplicacion.solo_problemas = false;
            }
        });
    });

    ui.add_space(tema.escala.s);
}

/// Una fila de la lista: una sola línea por reenvío.
///
/// Lo más prominente es el **nombre del túnel**, que es lo único que la
/// distingue de sus vecinas. El alias del host vive en la cabecera del grupo y
/// no se repite aquí; los extremos van abreviados y atenuados, porque se
/// consultan, no se leen.
#[allow(clippy::too_many_arguments)]
fn fila(
    aplicacion: &Aplicacion,
    ui: &mut egui::Ui,
    tunel: &Tunel,
    estado: &EstadoTunel,
    grupo: &GrupoHost,
    solo: bool,
    impar: bool,
    acciones: &mut Vec<Accion>,
) {
    let tema = *aplicacion.tema();
    let id = tunel.id();
    let seleccionada = aplicacion.detalle.as_deref() == Some(id.as_str());
    let marcada = aplicacion.seleccion.contains(&id);

    // El fondo alterna de base, y el estado lo pisa cuando hay algo que mirar.
    // La alternancia sirve para seguir la fila con la vista de un extremo al
    // otro; el color de estado, para que un túnel con problemas salte entre
    // quince. Son dos trabajos distintos y por eso conviven.
    let fondo = if seleccionada {
        tema.paleta.acento_suave
    } else if estado.estado.problematico() {
        tema.fondo_estado(estado.estado)
    } else if impar {
        tema.paleta.superficie
    } else if tema.paleta.oscuro {
        tema.paleta.elevado
    } else {
        tema.paleta.hover
    };

    egui::Frame::new()
        .fill(fondo)
        .inner_margin(tema.margen_simetrico(tema.escala.s, tema.escala.xs))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = tema.escala.s;

                let mut copia = marcada;
                if widgets::casilla(ui, &tema, &mut copia).clicked() {
                    acciones.push(Accion::Alternar(id.clone()));
                }
                widgets::indicador_estado(ui, &tema, estado.estado, iconos::PEQUENO);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = tema.escala.xs;
                    acciones_de_fila(
                        ui,
                        &tema,
                        tunel,
                        estado,
                        grupo.host.as_ref(),
                        seleccionada,
                        acciones,
                    );

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = tema.escala.s;
                        // El nombre, que es lo que distingue esta fila.
                        ui.label(
                            egui::RichText::new(tunel.reenvio.nombre_visible())
                                .size(tema.tipografia.cuerpo)
                                .color(if estado.estado == Estado::Activo {
                                    tema.paleta.texto
                                } else {
                                    tema.paleta.texto_secundario
                                }),
                        );
                        // Cuando el host tiene un solo reenvío no hay cabecera,
                        // así que el alias se dice aquí y solo aquí.
                        if solo {
                            ui.label(tema::tenue(&tema, &grupo.alias));
                        }
                        ui.label(tema::mono(&tema, tunel.reenvio.extremos_breves()));
                        // El motivo, en dos palabras. El texto completo va en
                        // el globo de ayuda: en la fila estorba y no se lee.
                        if estado.estado.problematico() {
                            if let Some(breve) = estado.motivo_breve() {
                                let etiqueta = ui.label(
                                    egui::RichText::new(breve)
                                        .size(tema.tipografia.micro)
                                        .color(tema.color_estado(estado.estado)),
                                );
                                if let Some(completo) = &estado.ultimo_error {
                                    etiqueta.on_hover_text(completo);
                                }
                            }
                        }
                    });
                });
            });
        });
}

/// Chips de metadatos del host: saltos, llave física, origen externo.
fn chips_del_host(ui: &mut egui::Ui, tema: &Tema, host: Option<&yonder::modelo::Host>) {
    let Some(host) = host else { return };

    if !host.saltos.is_empty() {
        let texto = if host.saltos.len() == 1 {
            format!("vía {}", host.saltos[0])
        } else {
            format!("{} saltos", host.saltos.len())
        };
        widgets::chip_neutro(ui, tema, Icono::RED, &texto)
            .on_hover_text(format!("ProxyJump: {}", host.saltos.join(" → ")));
    }

    if host.usa_clave_hardware() {
        widgets::chip(
            ui,
            tema,
            Some(Icono::LLAVE_FISICA),
            "FIDO2",
            tema.paleta.acento,
            tema.paleta.acento_suave,
        )
        .on_hover_text("Clave respaldada por hardware: habrá que tocar la llave física");
    }

    if let Origen::Ajeno(ruta) = &host.origen {
        widgets::chip_neutro(ui, tema, Icono::FICHERO_CLAVE, "externo").on_hover_text(format!(
            "Definido en {}\nSe puede activar, pero se edita a mano.",
            yonder::rutas::abreviar(ruta)
        ));
    }
}

/// Botones de la fila. El principal cambia según el estado.
fn acciones_de_fila(
    ui: &mut egui::Ui,
    tema: &Tema,
    tunel: &Tunel,
    estado: &EstadoTunel,
    host: Option<&yonder::modelo::Host>,
    desplegado: bool,
    acciones: &mut Vec<Accion>,
) {
    let id = tunel.id();
    let editable = host.map(|h| h.origen.editable()).unwrap_or(false);
    let alias = tunel.alias.clone();

    // Desplegador del acordeón. Es el único sitio desde el que se abre el
    // detalle: la fila entera no puede ser clicable sin dejar sordos a sus
    // propios botones (véase «tests/capas.rs»).
    if iconos::boton(
        ui,
        if desplegado {
            Icono::PLEGAR
        } else {
            Icono::DESPLEGAR
        },
        tema.paleta.texto_secundario,
        if desplegado {
            "Plegar el detalle"
        } else {
            "Desplegar el detalle"
        },
    )
    .clicked()
    {
        acciones.push(Accion::VerDetalle(id.clone()));
    }

    // Todo lo que no sea arrancar, parar o editar vive en un desplegable. Con
    // cinco iconos por fila y quince filas, el cromo tapaba el contenido; la
    // elegancia es quitar antes que añadir.
    ui.menu_image_button(
        iconos::imagen(Icono::MAS, iconos::NORMAL, tema.paleta.texto_secundario),
        |ui| {
            ui.set_min_width(220.0);
            if ui.button("Verificar la clave del host").clicked() {
                acciones.push(Accion::VerificarClave(alias.clone()));
                ui.close();
            }
            if ui.button("Instalar mi clave pública").clicked() {
                acciones.push(Accion::CopiarClave(alias.clone()));
                ui.close();
            }
            if editable {
                ui.separator();
                if ui.button("Eliminar el host").clicked() {
                    acciones.push(Accion::PedirEliminar(alias.clone()));
                    ui.close();
                }
            }
        },
    )
    .response
    .on_hover_text("Más acciones");

    if editable
        && iconos::boton(ui, Icono::EDITAR, tema.paleta.texto_secundario, "Editar").clicked()
    {
        acciones.push(Accion::Editar(tunel.alias.clone()));
    }

    ui.add_space(tema.escala.xs);

    // Acciones secundarias, solo cuando el estado las pide.
    match estado.estado {
        Estado::Reintentando => {
            if widgets::boton_de_fila(ui, tema, Icono::RAYO, tema.paleta.info, "Ahora")
                .on_hover_text("Reintentar ya, sin esperar al siguiente intento")
                .clicked()
            {
                acciones.push(Accion::Reintentar(id.clone()));
            }
        }
        Estado::Degradado
            if widgets::boton_de_fila(
                ui,
                tema,
                Icono::REINTENTAR,
                tema.paleta.aviso,
                "Reparar",
            )
            .on_hover_text("Rehacer el reenvío sin cerrar la conexión")
            .clicked() =>
        {
            acciones.push(Accion::Reintentar(id.clone()));
        }
        _ => {}
    }

    // El interruptor del túnel. UN solo control, siempre en el mismo sitio, que
    // alterna entre arrancar y parar según lo que el túnel esté haciendo.
    //
    // Antes había un botón distinto por estado, y en algunos ni siquiera
    // aparecía el de parar: había que adivinar qué hacía el icono de turno. Un
    // interruptor no se adivina, se lee.
    let arriba = !matches!(estado.estado, Estado::Definido | Estado::Fallido);
    let (icono, color, texto, ayuda) = if arriba {
        (
            Icono::PARAR,
            tema.paleta.error,
            "Parar",
            "Cerrar este reenvío. Si es el último del host, se cierra también la conexión",
        )
    } else {
        (
            Icono::ARRANCAR,
            tema.paleta.exito,
            "Levantar",
            "Abrir la conexión y establecer este reenvío",
        )
    };

    if widgets::boton_de_fila(ui, tema, icono, color, texto)
        .on_hover_text(ayuda)
        .clicked()
    {
        acciones.push(if arriba {
            Accion::Bajar(id)
        } else {
            Accion::Levantar(id)
        });
    }
}

/// Pantalla de bienvenida cuando no hay nada definido.
fn pantalla_vacia(aplicacion: &mut Aplicacion, ui: &mut egui::Ui) {
    let tema = *aplicacion.tema();
    let hay_importables = !aplicacion.instantanea().importables.is_empty();

    ui.add_space(tema.escala.xxl);
    ui.vertical_centered(|ui| {
        iconos::mostrar(
            ui,
            Icono::TUNEL,
            56.0,
            tema.paleta.acento.gamma_multiply(0.7),
        );
        ui.add_space(tema.escala.m);
        ui.label(
            egui::RichText::new("Todavía no hay ningún túnel")
                .size(tema.tipografia.cabecera)
                .color(tema.paleta.texto),
        );
        ui.add_space(tema.escala.s);
        ui.label(tema::secundario(
            &tema,
            "Los túneles se guardan en ~/.ssh/config.d/yonder.conf, así que\n\
             también funcionarán con ssh, scp, rsync y VS Code Remote.",
        ));
        ui.add_space(tema.escala.xl);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = tema.escala.s;
            // Centrado manual: el ancho de los dos botones más su separación.
            let ancho = if hay_importables { 300.0 } else { 150.0 };
            ui.add_space((ui.available_width() - ancho).max(0.0) / 2.0);

            if widgets::boton_principal(ui, &tema, Icono::ANADIR, "Crear el primero", true)
                .clicked()
            {
                aplicacion.modal = Modal::Editor(editor::EstadoEditor::nuevo());
            }
            if hay_importables
                && widgets::boton_secundario(
                    ui,
                    &tema,
                    Icono::IMPORTAR,
                    "Importar existentes",
                    true,
                )
                .clicked()
            {
                aplicacion.modal = Modal::Importar;
            }
        });
    });
}

/// Panel derecho con el detalle del túnel seleccionado.
pub fn panel_detalle(aplicacion: &mut Aplicacion, ui: &mut egui::Ui, id: &str) {
    let tema = *aplicacion.tema();
    let Some(tunel) = aplicacion
        .instantanea()
        .tuneles
        .iter()
        .find(|t| t.id() == id)
        .cloned()
    else {
        aplicacion.detalle = None;
        return;
    };
    let estado = aplicacion
        .instantanea()
        .estado(id)
        .cloned()
        .unwrap_or_else(|| EstadoTunel::nuevo(id));
    let host = aplicacion.host(&tunel.alias).cloned();

    // Ni título ni botón de cerrar: los pone el marco de la ventana.
    ui.add_space(tema.escala.s);
    ui.horizontal(|ui| {
        widgets::etiqueta_estado(ui, &tema, estado.estado);
    });
    ui.add_space(tema.escala.xs);
    ui.label(tema::tenue(&tema, estado.estado.explicacion()));

    ui.add_space(tema.escala.m);
    widgets::divisor(ui, &tema);

    // Sin `ScrollArea` propio: esto se despliega dentro del de la lista, y dos
    // anidados se pelean por la rueda del ratón. Al crecer empuja hacia abajo
    // los túneles siguientes, que es el comportamiento de un acordeón.
    {
        {
            widgets::cabecera_seccion(ui, &tema, "Reenvío");
            widgets::propiedad(ui, &tema, "Tipo", tunel.reenvio.tipo.etiqueta());
            widgets::propiedad(ui, &tema, "Escucha", &tunel.reenvio.escucha.to_string());
            if let Some(destino) = &tunel.reenvio.destino {
                widgets::propiedad(ui, &tema, "Destino", &destino.to_string());
            }
            if tunel.reenvio.escucha.expuesto() {
                ui.add_space(tema.escala.xs);
                widgets::caja_aviso(
                    ui,
                    &tema,
                    false,
                    "Este reenvío escucha en todas las interfaces, no solo en el bucle local: \
                     cualquiera de tu red puede usarlo.",
                );
            }

            if let Some(host) = &host {
                widgets::cabecera_seccion(ui, &tema, "Host");
                widgets::propiedad(ui, &tema, "Destino", host.destino());
                if let Some(usuario) = &host.usuario {
                    widgets::propiedad(ui, &tema, "Usuario", usuario);
                }
                widgets::propiedad(ui, &tema, "Puerto", &host.puerto.unwrap_or(22).to_string());
                if !host.saltos.is_empty() {
                    widgets::propiedad(ui, &tema, "Saltos", &host.saltos.join(" → "));
                }
                for identidad in &host.identidades {
                    widgets::propiedad(ui, &tema, "Clave", identidad);
                }
                widgets::propiedad(
                    ui,
                    &tema,
                    "Origen",
                    &match &host.origen {
                        Origen::Propio => "fichero propio".to_string(),
                        Origen::Ajeno(ruta) => yonder::rutas::abreviar(ruta),
                    },
                );
            }

            widgets::cabecera_seccion(ui, &tema, "Comprobación de salud");
            widgets::propiedad(ui, &tema, "Sonda", &tunel.reenvio.salud.etiqueta());
            ui.label(tema::tenue(&tema, tunel.reenvio.salud.explicacion()));
            if !tunel.reenvio.salud.atraviesa_el_tunel() && tunel.reenvio.tipo.escucha_en_local() {
                ui.add_space(tema.escala.xs);
                widgets::caja_aviso(
                    ui,
                    &tema,
                    false,
                    "Solo se comprueba que el puerto esté abierto. Si el reenvío apunta a \
                     donde el servicio remoto no escucha, el túnel se verá activo y no \
                     transportará nada. Cámbialo a «banner» o «HTTP» desde el editor.",
                );
            }

            widgets::cabecera_seccion(ui, &tema, "Ejecución");
            match estado.pid_maestro {
                Some(pid) => widgets::propiedad(ui, &tema, "PID del maestro", &pid.to_string()),
                None => widgets::propiedad(ui, &tema, "PID del maestro", "sin maestro"),
            }
            if let Ok(socket) = yonder::rutas::socket_control(&tunel.alias) {
                widgets::propiedad(ui, &tema, "Socket", &yonder::rutas::abreviar(&socket));
            }
            widgets::propiedad(
                ui,
                &tema,
                "En este estado desde",
                &widgets::duracion_legible(estado.antiguedad()),
            );

            if let Some(error) = &estado.ultimo_error {
                widgets::cabecera_seccion(ui, &tema, "Último error");
                widgets::caja_aviso(ui, &tema, true, error);
            }

            widgets::cabecera_seccion(ui, &tema, "Equivalente en la terminal");
            ui.label(tema::tenue(
                &tema,
                "En este equipo, porque la definición está en el fichero de \
                 configuración. Funciona sin la aplicación abierta, que es justo \
                 el objetivo:",
            ));
            ui.add_space(tema.escala.xs);
            modales::bloque_codigo(ui, &tema, &format!("ssh {}", tunel.alias));

            // La orden autocontenida, para llevársela fuera. Va aquí y no en un
            // sitio más visible porque es el caso raro; pero cuando toca, se
            // está en un servidor sin escritorio y sin este fichero, y entonces
            // es lo único que sirve.
            if let Some(host) = &host {
                ui.add_space(tema.escala.m);
                ui.label(tema::tenue(
                    &tema,
                    "En cualquier otra máquina, que no tiene esta configuración. \
                     Abre solo este túnel, en primer plano; se cierra con Ctrl-C:",
                ));
                ui.add_space(tema.escala.xs);
                modales::bloque_codigo(ui, &tema, &host.orden_ssh_manual(&tunel.reenvio));
            }

            ui.add_space(tema.escala.m);
            widgets::divisor(ui, &tema);
            ui.add_space(tema.escala.m);

            // El valor real vive en la base de datos y el supervisor lo publica
            // en cada instantánea: la casilla refleja el estado guardado, no
            // una copia local que pudiera desincronizarse.
            let mut autoarranque = aplicacion
                .instantanea()
                .autoarranque
                .iter()
                .any(|marcado| marcado == id);
            ui.horizontal(|ui| {
                if widgets::casilla(ui, &tema, &mut autoarranque).clicked() {
                    aplicacion.ordenar(Orden::FijarAutoarranque(id.to_string(), autoarranque));
                }
                ui.label(tema::cuerpo(&tema, "Levantar al abrir la aplicación"));
            });
        }
    }
}

/// Aplica las acciones acumuladas durante el recorrido de la lista.
fn aplicar(aplicacion: &mut Aplicacion, acciones: Vec<Accion>) {
    for accion in acciones {
        match accion {
            Accion::Alternar(id) => {
                if !aplicacion.seleccion.remove(&id) {
                    aplicacion.seleccion.insert(id);
                }
            }
            Accion::VerDetalle(id) => {
                aplicacion.detalle = if aplicacion.detalle.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id)
                };
            }
            Accion::Levantar(id) => aplicacion.ordenar(Orden::Levantar(id)),
            Accion::Bajar(id) => aplicacion.ordenar(Orden::Bajar(id)),
            Accion::Reintentar(id) => aplicacion.ordenar(Orden::ReintentarYa(id)),
            Accion::Editar(alias) => {
                if let Some(host) = aplicacion.host(&alias).cloned() {
                    aplicacion.modal = Modal::Editor(editor::EstadoEditor::de(&host));
                }
            }
            Accion::PedirEliminar(alias) => {
                aplicacion.modal = Modal::Confirmacion {
                    titulo: format!("¿Eliminar «{alias}»?"),
                    texto: "Se borrará el bloque Host de ~/.ssh/config.d/yonder.conf con todos \
                         sus reenvíos. Si el túnel está activo, se bajará antes.\n\n\
                         El resto del fichero, incluidos comentarios y directivas que la \
                         aplicación no gestiona, queda intacto."
                        .to_string(),
                    etiqueta_aceptar: "Eliminar".to_string(),
                    orden: Some(Orden::EliminarHost(alias)),
                };
            }
            Accion::VerificarClave(alias) => {
                aplicacion.modal =
                    Modal::ClaveHost(modales::EstadoClaveHost::nuevo(aplicacion, &alias));
            }
            Accion::CopiarClave(alias) => {
                aplicacion.modal = Modal::CopiarClave(modales::EstadoCopiarClave::nuevo(&alias));
            }
        }
    }
}
