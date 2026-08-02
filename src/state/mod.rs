//! Motor de estado: reconstruye la verdad y actúa sobre ella.
//!
//! El motor es **síncrono y sin hilos**. La CLI lo usa directamente y el
//! supervisor (§3.3) lo envuelve en un hilo con reintentos. Que la lógica viva
//! aquí y no en el supervisor es lo que permite que la fase 1 sea usable a
//! diario sin interfaz gráfica.

pub mod db;
pub mod machine;
pub mod supervisor;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::Catalogo;
use crate::error::{Error, Resultado};
use crate::modelo::{Host, Tunel};
use crate::net::probe::Escuchas;
use crate::net::salud::{self, Veredicto};
use crate::ssh::{control, forward, Entorno, Maestro, OpcionesMaestro};

pub use db::{BaseDeDatos, RegistroTunel};
pub use machine::{Estado, EstadoTunel};
pub use supervisor::{Instantanea, Orden, PoliticaReintento, Supervisor};

/// Cadencia por defecto de la sonda profunda de salud.
///
/// El guion que esto sustituye comprobaba cada dos minutos. Treinta segundos da
/// margen para enterarse pronto sin martillear el servicio del otro lado.
pub const INTERVALO_SALUD_POR_DEFECTO: Duration = Duration::from_secs(30);

/// Motor de la aplicación.
pub struct Motor {
    pub catalogo: Catalogo,
    pub base: BaseDeDatos,
    pub entorno: Entorno,
    pub opciones: OpcionesMaestro,
    /// Cada cuánto se sondea la salud profunda de cada túnel.
    ///
    /// Las sondas de `banner` y `http` abren conexiones reales a través del
    /// túnel, así que tienen efecto en el servicio del otro lado. Se ejecutan a
    /// esta cadencia y no en cada latido; la comprobación barata —¿escucha el
    /// puerto?— sigue corriendo cada segundo.
    pub intervalo_salud: Duration,
    estados: HashMap<String, EstadoTunel>,
    /// Última vez que se preguntó el PID a cada maestro.
    ultimo_check: HashMap<String, Instant>,
    /// Última sonda profunda por túnel, con su veredicto.
    ultima_salud: HashMap<String, (Instant, Veredicto)>,
}

impl std::fmt::Debug for Motor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Motor")
            .field("yonder", &self.estados.len())
            .finish_non_exhaustive()
    }
}

impl Motor {
    pub fn nuevo(entorno: Entorno) -> Resultado<Motor> {
        let catalogo = Catalogo::cargar()?;
        let base = BaseDeDatos::abrir()?;
        let mut motor = Motor {
            catalogo,
            base,
            entorno,
            opciones: OpcionesMaestro::default(),
            intervalo_salud: INTERVALO_SALUD_POR_DEFECTO,
            estados: HashMap::new(),
            ultimo_check: HashMap::new(),
            ultima_salud: HashMap::new(),
        };
        motor.sincronizar_registros()?;
        Ok(motor)
    }

    /// Vuelve a leer la configuración del disco.
    pub fn recargar(&mut self) -> Resultado<()> {
        self.catalogo = Catalogo::cargar()?;
        self.sincronizar_registros()
    }

    /// Da de alta los túneles nuevos y purga los que ya no existen.
    fn sincronizar_registros(&mut self) -> Resultado<()> {
        let ids: Vec<String> = self.catalogo.tuneles().iter().map(|t| t.id()).collect();
        self.base.purgar(&ids)?;
        self.estados.retain(|id, _| ids.contains(id));
        for id in &ids {
            self.estados
                .entry(id.clone())
                .or_insert_with(|| EstadoTunel::nuevo(id.clone()));
        }
        Ok(())
    }

    /// Túneles que la interfaz debe mostrar.
    ///
    /// Los del fichero propio siempre; los externos solo si el usuario los ha
    /// importado, es decir, ha dicho que quiere verlos (§3.1).
    pub fn tuneles_visibles(&self) -> Vec<Tunel> {
        let mut visibles: Vec<Tunel> = self
            .catalogo
            .propios
            .iter()
            .flat_map(|h| h.tuneles())
            .collect();
        for host in &self.catalogo.externos {
            if self.base.externo_visible(&host.alias).unwrap_or(false) {
                visibles.extend(host.tuneles());
            }
        }
        // Orden guardado por el usuario; los nuevos, al final.
        let orden: HashMap<String, i64> = self
            .base
            .todos()
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.id, r.orden))
            .collect();
        visibles.sort_by_key(|t| orden.get(&t.id()).copied().unwrap_or(i64::MAX));
        visibles
    }

    /// Hosts externos aún no importados: candidatos de la pantalla de importación.
    pub fn importables(&self) -> Vec<&Host> {
        self.catalogo
            .externos
            .iter()
            .filter(|h| !self.base.externo_visible(&h.alias).unwrap_or(false))
            .collect()
    }

    pub fn importar(&self, alias: &str) -> Resultado<()> {
        self.base.fijar_externo_visible(alias, true)
    }

    pub fn tunel(&self, id: &str) -> Resultado<Tunel> {
        self.catalogo
            .tunel(id)
            .ok_or_else(|| Error::TunelDesconocido(id.to_string()))
    }

    fn host_de(&self, tunel: &Tunel) -> Resultado<&Host> {
        self.catalogo
            .host(&tunel.alias)
            .ok_or_else(|| Error::HostDesconocido(tunel.alias.clone()))
    }

    pub fn estado(&self, id: &str) -> EstadoTunel {
        self.estados
            .get(id)
            .cloned()
            .unwrap_or_else(|| EstadoTunel::nuevo(id))
    }

    pub fn estados(&self) -> Vec<EstadoTunel> {
        self.tuneles_visibles()
            .iter()
            .map(|t| self.estado(&t.id()))
            .collect()
    }

    fn estado_mut(&mut self, id: &str) -> &mut EstadoTunel {
        self.estados
            .entry(id.to_string())
            .or_insert_with(|| EstadoTunel::nuevo(id))
    }

    /// Sustituye el estado de un túnel. Lo usa el supervisor para anotar el
    /// contador de reintentos y el momento del siguiente intento.
    pub fn fijar_estado(&mut self, estado: EstadoTunel) {
        self.estados.insert(estado.id.clone(), estado);
    }

    /// Cambia de estado contabilizando el tiempo que estuvo activo.
    fn transitar(&mut self, id: &str, siguiente: Estado) {
        let anterior = self.estado(id);
        if anterior.estado == siguiente {
            return;
        }
        if anterior.estado == Estado::Activo {
            let segundos = anterior.antiguedad().as_secs() as i64;
            let _ = self.base.sumar_tiempo_activo(id, segundos);
        }
        self.estado_mut(id).transitar(siguiente);
    }

    /// Levanta un túnel: abre el maestro si hace falta y activa el reenvío.
    ///
    /// Es idempotente. Si el maestro ya estaba abierto no se reautentica: es
    /// justo lo que hace que activar un segundo túnel del mismo host sea
    /// instantáneo (§3.2).
    pub fn levantar(&mut self, id: &str) -> Resultado<()> {
        let tunel = self.tunel(id)?;
        let host = self.host_de(&tunel)?.clone();

        // Si `ssh` no puede ver el alias, abrir el maestro fallaría con un
        // «Could not resolve hostname», que no le dice nada a nadie. Vale más
        // detectarlo aquí y nombrar la causa (§12, criterio 5).
        self.comprobar_visible_para_ssh(&host)?;

        self.base.fijar_deseado(id, true)?;
        // El veredicto anterior es de antes de la reconexión: no vale.
        self.olvidar_salud(id);
        self.transitar(id, Estado::Conectando);

        let resultado = (|| -> Resultado<u32> {
            let maestro = Maestro::de(&tunel.alias)?;
            let pid = maestro.abrir(&host, &self.opciones, &self.entorno)?;
            forward::activar(&maestro.control, &tunel.reenvio, &self.entorno)?;
            Ok(pid)
        })();

        match resultado {
            Ok(pid) => {
                self.base.apuntar_exito(id)?;
                let escuchando = forward::confirmado(&tunel.reenvio);
                // Se sondea aquí mismo. Si el reenvío nace zombi —un destino
                // que apunta a donde no hay servicio— hay que verlo ahora y no
                // dentro de medio minuto con un punto verde de por medio.
                let sano = escuchando && self.salud_de(&tunel, escuchando);
                self.estado_mut(id).pid_maestro = Some(pid);
                self.transitar(
                    id,
                    if sano {
                        Estado::Activo
                    } else {
                        Estado::Degradado
                    },
                );
                if !sano {
                    let motivo = self
                        .veredicto(id)
                        .and_then(|v| v.motivo())
                        .unwrap_or("el puerto local no está en escucha")
                        .to_string();
                    self.estado_mut(id).ultimo_error = Some(motivo);
                }
                Ok(())
            }
            Err(e) => {
                self.base.apuntar_fallo(id)?;
                let estado = self.estado_mut(id);
                estado.ultimo_error = Some(mensaje_completo(&e));
                estado.pid_maestro = None;
                self.transitar(id, Estado::Reintentando);
                Err(e)
            }
        }
    }

    /// Baja un túnel. Cierra el maestro si era el último de su alias.
    pub fn bajar(&mut self, id: &str) -> Resultado<()> {
        let tunel = self.tunel(id)?;
        self.base.fijar_deseado(id, false)?;
        self.transitar(id, Estado::Cerrando);

        let maestro = Maestro::de(&tunel.alias)?;
        if maestro.activo() {
            if let Err(e) = forward::cancelar(&maestro.control, &tunel.reenvio, &self.entorno) {
                // Que el reenvío ya no estuviera no es un fallo: el objetivo
                // era que dejara de estar y lo está.
                tracing::warn!(id = %id, "el reenvío no se pudo cancelar limpiamente: {e}");
            }
            if !self.quedan_deseados_en(&tunel.alias, id) {
                maestro.cerrar()?;
            }
        } else {
            maestro.control.retirar_restos();
        }

        self.estado_mut(id).pid_maestro = None;
        self.transitar(id, Estado::Definido);
        Ok(())
    }

    /// Comprueba que `ssh` puede resolver el alias antes de intentar nada.
    ///
    /// Un host del fichero propio solo existe para `ssh` si `~/.ssh/config`
    /// lo incluye. Sin esa línea, `ssh <alias>` trata el alias como un nombre
    /// de máquina, intenta resolverlo por DNS y falla con un mensaje que no
    /// menciona la causa real. Es el mismo motivo por el que el criterio 7 de
    /// §12 valida toda la decisión de §3.1.
    ///
    /// Solo se comprueba para hosts propios: los externos ya están en un
    /// fichero que `ssh` lee por su cuenta.
    fn comprobar_visible_para_ssh(&self, host: &Host) -> Resultado<()> {
        if !host.origen.editable() {
            return Ok(());
        }
        match crate::config::estado_include() {
            Ok(crate::config::EstadoInclude::Presente) => Ok(()),
            Ok(_) => Err(Error::AliasInvisible {
                alias: host.alias.clone(),
            }),
            // Si no se puede saber, se deja pasar: `ssh` dirá lo que tenga que
            // decir. Bloquear por una comprobación que ha fallado sería peor.
            Err(e) => {
                tracing::warn!("no se pudo comprobar la línea Include: {e}");
                Ok(())
            }
        }
    }

    /// `true` si quedan otros túneles del mismo alias que el usuario quiere arriba.
    fn quedan_deseados_en(&self, alias: &str, excluyendo: &str) -> bool {
        self.catalogo
            .tuneles()
            .iter()
            .filter(|t| t.alias == alias && t.id() != excluyendo)
            .any(|t| self.base.tunel(&t.id()).map(|r| r.deseado).unwrap_or(false))
    }

    /// Vuelve a pedir un reenvío que se ha caído con el maestro todavía vivo.
    ///
    /// Es la salida barata de `Degradado`: no hay que reautenticar, solo repetir
    /// el `-O forward`. Si eso no lo arregla, [`Motor::reparar_a_fondo`] escala.
    pub fn reparar(&mut self, id: &str) -> Resultado<()> {
        let tunel = self.tunel(id)?;
        let maestro = Maestro::de(&tunel.alias)?;
        if !maestro.activo() {
            return Err(Error::MaestroInactivo(tunel.alias.clone()));
        }
        // Puede haber quedado a medias: cancelar primero evita el «forward
        // already requested» cuando ssh cree que sigue en pie.
        let _ = forward::cancelar(&maestro.control, &tunel.reenvio, &self.entorno);
        forward::activar(&maestro.control, &tunel.reenvio, &self.entorno)?;

        // Tras rehacer el reenvío hay que volver a sondear: quedarse con el
        // veredicto viejo lo dejaría en Degradado aunque ya funcionase.
        self.olvidar_salud(id);
        let escuchando = forward::confirmado(&tunel.reenvio);
        if escuchando && self.salud_de(&tunel, escuchando) {
            self.transitar(id, Estado::Activo);
            return Ok(());
        }

        Err(Error::OrdenFallida {
            orden: format!("ssh -O forward {}", tunel.reenvio.argumento()),
            codigo: 0,
            salida: match self.veredicto(id).and_then(|v| v.motivo()) {
                Some(motivo) => format!("el reenvío se rehízo y sigue sin responder: {motivo}"),
                None => "el reenvío se aceptó pero el puerto local no escucha".to_string(),
            },
        })
    }

    /// Cierra el maestro y lo vuelve a abrir con todos sus túneles deseados.
    ///
    /// Es el escalón siguiente cuando repetir el `-O forward` no arregla nada:
    /// el maestro sigue respondiendo al socket de control pero su multiplexor
    /// está atascado. El guion que esto sustituye lo resolvía matando el proceso
    /// `ssh` a lo bruto; aquí se cierra limpiamente y se reabre, que además
    /// recupera de una vez todos los reenvíos del mismo alias.
    pub fn reparar_a_fondo(&mut self, id: &str) -> Resultado<()> {
        let tunel = self.tunel(id)?;
        let alias = tunel.alias.clone();
        tracing::warn!(alias = %alias, "el reenvío no se recupera: se reabre la conexión maestra");

        let maestro = Maestro::de(&alias)?;
        maestro.cerrar()?;

        // Todos los túneles del alias han caído con el maestro; se recuperan
        // los que el usuario quiere arriba, empezando por el que dio el aviso.
        let deseados: Vec<String> = self
            .catalogo
            .tuneles()
            .iter()
            .filter(|t| t.alias == alias)
            .map(|t| t.id())
            .filter(|otro| otro == id || self.base.tunel(otro).map(|r| r.deseado).unwrap_or(false))
            .collect();

        let mut primer_error = None;
        for otro in deseados {
            self.olvidar_salud(&otro);
            if let Err(e) = self.levantar(&otro) {
                if primer_error.is_none() {
                    primer_error = Some(e);
                }
            }
        }
        match primer_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Reconstruye el estado observando la realidad (§3.6).
    ///
    /// Esta función no actúa: solo mira. Quien decide reintentar es el
    /// supervisor. Separarlo permite que la CLI observe sin efectos.
    pub fn observar(&mut self) {
        let escuchas = Escuchas::tomar();
        let tuneles = self.catalogo.tuneles();

        // Un `-O check` por alias es un proceso; una conexión al socket no lo
        // es. Se usa la barata en cada pasada y la cara solo de vez en cuando.
        let mut maestros: HashMap<String, bool> = HashMap::new();
        for tunel in &tuneles {
            maestros.entry(tunel.alias.clone()).or_insert_with(|| {
                crate::rutas::socket_control(&tunel.alias)
                    .map(|ruta| control::Control::socket_vivo(&ruta))
                    .unwrap_or(false)
            });
        }

        for tunel in &tuneles {
            let id = tunel.id();
            let deseado = self.base.tunel(&id).map(|r| r.deseado).unwrap_or(false);
            let maestro_vivo = maestros.get(&tunel.alias).copied().unwrap_or(false);
            let escuchando = forward::confirmado_en(&tunel.reenvio, &escuchas);
            let actual = self.estado(&id).estado;

            // La sonda profunda solo tiene sentido si el usuario lo quiere
            // arriba, el maestro vive y el puerto ya escucha. En cualquier otro
            // caso no aportaría nada y sí abriría conexiones para nada.
            let sano = if deseado && maestro_vivo && escuchando {
                self.salud_de(tunel, escuchando)
            } else {
                true
            };

            let siguiente = match (deseado, maestro_vivo, escuchando && sano) {
                // El usuario no lo quiere arriba: lo que se vea es irrelevante.
                (false, _, _) if actual != Estado::Cerrando => Estado::Definido,
                (false, _, _) => actual,

                // Lo quiere arriba y todo está en su sitio.
                (true, true, true) => Estado::Activo,

                // Maestro vivo y el túnel no responde: el estado crítico de §4.
                // Puede ser que el puerto no escuche o que escuche y no
                // transporte —el zombi—; el motivo se apunta abajo.
                (true, true, false) => match actual {
                    Estado::Conectando | Estado::Cerrando => actual,
                    _ => Estado::Degradado,
                },

                // Sin maestro: hay que reconectar.
                (true, false, _) => match actual {
                    Estado::Conectando
                    | Estado::Reintentando
                    | Estado::Fallido
                    | Estado::Cerrando => actual,
                    _ => Estado::Reintentando,
                },
            };

            if siguiente != actual {
                // Observar no es transitar: al arrancar, la memoria dice
                // `Definido` y la realidad puede decir cualquier cosa.
                if actual == Estado::Activo {
                    let segundos = self.estado(&id).antiguedad().as_secs() as i64;
                    let _ = self.base.sumar_tiempo_activo(&id, segundos);
                }
                self.estado_mut(&id).reconstruir(siguiente);
            }

            // Motivo del degradado. Sin esto, un zombi y un puerto caído se
            // verían igual en la interfaz, y no se arreglan igual.
            if siguiente == Estado::Degradado {
                let motivo = self
                    .ultima_salud
                    .get(&id)
                    .and_then(|(_, veredicto)| veredicto.motivo())
                    .unwrap_or("el puerto local no está en escucha")
                    .to_string();
                self.estado_mut(&id).ultimo_error = Some(motivo);
            }

            self.refrescar_pid(tunel, maestro_vivo);
        }
    }

    /// Sondea la salud profunda de un túnel, con memoria y cadencia propia.
    ///
    /// Devuelve `true` si el túnel se considera sano. Entre sondas se reutiliza
    /// el último veredicto: las sondas abren conexiones reales a través del
    /// túnel y no se pueden lanzar en cada latido.
    fn salud_de(&mut self, tunel: &Tunel, escuchando: bool) -> bool {
        let id = tunel.id();

        // La comprobación barata ya está hecha y es la única que aplica.
        if !tunel.reenvio.salud.atraviesa_el_tunel() {
            return escuchando;
        }
        // Un reenvío remoto escucha en la otra punta: no hay nada que sondear.
        if !tunel.reenvio.tipo.escucha_en_local() {
            return escuchando;
        }

        let toca = self
            .ultima_salud
            .get(&id)
            .map(|(momento, _)| momento.elapsed() >= self.intervalo_salud)
            .unwrap_or(true);

        if !toca {
            return self
                .ultima_salud
                .get(&id)
                .map(|(_, veredicto)| veredicto.sano())
                .unwrap_or(true);
        }

        let veredicto = salud::sondear(
            &tunel.reenvio.salud,
            tunel.reenvio.escucha.direccion_efectiva(),
            tunel.reenvio.escucha.puerto,
            escuchando,
        );
        if !veredicto.sano() {
            tracing::warn!(
                id = %id,
                sonda = %tunel.reenvio.salud.etiqueta(),
                "el túnel no responde: {}",
                veredicto.motivo().unwrap_or("motivo desconocido")
            );
        }
        let sano = veredicto.sano();
        self.ultima_salud.insert(id, (Instant::now(), veredicto));
        sano
    }

    /// Obliga a que la siguiente observación vuelva a sondear ese túnel.
    ///
    /// Se usa tras reparar: dar por bueno el veredicto anterior dejaría el
    /// túnel en `Degradado` hasta la siguiente ronda aunque ya funcione.
    fn olvidar_salud(&mut self, id: &str) {
        self.ultima_salud.remove(id);
    }

    /// Último veredicto de salud conocido, si lo hay.
    pub fn veredicto(&self, id: &str) -> Option<&Veredicto> {
        self.ultima_salud.get(id).map(|(_, veredicto)| veredicto)
    }

    /// Actualiza el PID del maestro sin gastar un proceso en cada pasada.
    fn refrescar_pid(&mut self, tunel: &Tunel, maestro_vivo: bool) {
        let id = tunel.id();
        if !maestro_vivo {
            self.estado_mut(&id).pid_maestro = None;
            return;
        }
        let toca = self
            .ultimo_check
            .get(&tunel.alias)
            .map(|momento| momento.elapsed() > std::time::Duration::from_secs(30))
            .unwrap_or(true);
        if !toca {
            return;
        }
        self.ultimo_check
            .insert(tunel.alias.clone(), Instant::now());
        if let Ok(maestro) = Maestro::de(&tunel.alias) {
            if let Ok(pid) = maestro.pid() {
                self.estado_mut(&id).pid_maestro = pid;
            }
        }
    }

    /// Túneles marcados para levantarse al arrancar.
    pub fn de_autoarranque(&self) -> Vec<String> {
        self.tuneles_visibles()
            .iter()
            .map(|t| t.id())
            .filter(|id| self.base.tunel(id).map(|r| r.autoarranque).unwrap_or(false))
            .collect()
    }

    /// Limpieza de sockets huérfanos al arrancar (§3.6).
    pub fn limpiar_al_arrancar(&mut self) -> Resultado<control::Limpieza> {
        let informe = control::limpiar_huerfanos()?;
        self.observar();
        Ok(informe)
    }
}

/// Mensaje de error con su sugerencia, si la tiene.
pub fn mensaje_completo(error: &Error) -> String {
    match error.sugerencia() {
        Some(sugerencia) => format!("{error}\n{sugerencia}"),
        None => error.to_string(),
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use crate::modelo::{Extremo, Reenvio};

    #[test]
    fn el_registro_recuerda_la_intencion_entre_arranques() {
        let base = BaseDeDatos::en_memoria().unwrap();
        let tunel = Tunel {
            alias: "preprod".into(),
            reenvio: Reenvio::local(
                Extremo::solo_puerto(3000),
                Extremo::nuevo("localhost", 3000),
            ),
        };
        base.fijar_deseado(&tunel.id(), true).unwrap();

        // Sin esta marca no habría forma de distinguir «nunca lo levanté» de
        // «se me ha caído», que es exactamente la diferencia entre Definido y
        // Degradado tras reabrir la ventana.
        assert!(base.tunel(&tunel.id()).unwrap().deseado);
    }
}
