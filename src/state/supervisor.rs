//! Supervisión con reintento y retardo exponencial (§3.3).
//!
//! Nada de `autossh`. `ServerAliveInterval` + `ServerAliveCountMax` detectan la
//! caída y este supervisor decide cuándo volver a intentarlo: base 2 s, techo
//! 60 s y *jitter* aleatorio del ±20 % para que veinte túneles que se caen a la
//! vez —una VPN que se corta— no vuelvan todos en el mismo milisegundo.
//!
//! El supervisor vive en un hilo y **no es un demonio**: muere con la ventana.
//! Los maestros no, porque están desprendidos (§3.6).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rand::Rng;

use crate::error::Resultado;
use crate::modelo::{Host, Tunel};
use crate::net::salud::{self, Veredicto};
use crate::ssh::Maestro;

use super::machine::{Estado, EstadoTunel};
use super::{conectar, mensaje_completo, Conexion, Motor, PlanDeConexion};

/// Reparaciones suaves antes de reabrir la conexión maestra entera.
///
/// Rehacer el `-O forward` arregla el caso normal sin reautenticar. Si dos
/// intentos seguidos no bastan, el problema no es el reenvío sino el
/// multiplexor del maestro, y hay que cerrarlo y volver a abrirlo.
const REPARACIONES_ANTES_DE_REABRIR: u32 = 2;

/// Cadencia del latido del supervisor.
///
/// Un segundo es barato: la comprobación de cada latido es una lectura de
/// `/proc` y una conexión a un socket Unix por alias. Nada de procesos.
pub const LATIDO: Duration = Duration::from_millis(1000);

/// Política de reintento.
#[derive(Debug, Clone, Copy)]
pub struct PoliticaReintento {
    pub base: Duration,
    pub techo: Duration,
    /// Proporción de variación aleatoria, 0.20 = ±20 %.
    pub jitter: f64,
    /// Intentos antes de darse por vencido. 0 = sin límite.
    pub maximo_intentos: u32,
}

impl Default for PoliticaReintento {
    fn default() -> Self {
        PoliticaReintento {
            base: Duration::from_secs(2),
            techo: Duration::from_secs(60),
            jitter: 0.20,
            maximo_intentos: 10,
        }
    }
}

impl PoliticaReintento {
    /// Retardo del intento `n` (1 es el primer reintento).
    pub fn retardo(&self, intento: u32) -> Duration {
        let exponente = intento.saturating_sub(1).min(16);
        let crudo = self.base.saturating_mul(1u32 << exponente);
        let acotado = crudo.min(self.techo);
        self.aplicar_jitter(acotado)
    }

    fn aplicar_jitter(&self, retardo: Duration) -> Duration {
        if self.jitter <= 0.0 {
            return retardo;
        }
        let factor = rand::thread_rng().gen_range(-self.jitter..=self.jitter);
        let segundos = retardo.as_secs_f64() * (1.0 + factor);
        Duration::from_secs_f64(segundos.max(0.1))
    }

    pub fn agotado(&self, intento: u32) -> bool {
        self.maximo_intentos > 0 && intento >= self.maximo_intentos
    }
}

/// Órdenes que la interfaz envía al supervisor.
#[derive(Debug, Clone)]
pub enum Orden {
    Levantar(String),
    Bajar(String),
    /// Levanta varios de golpe: el botón «iniciar seleccionados».
    LevantarVarios(Vec<String>),
    BajarVarios(Vec<String>),
    /// Reintenta ya, sin esperar el retardo. Es el botón de reintentar.
    ReintentarYa(String),
    /// Vuelve a leer `~/.ssh/config.d/yonder.conf`.
    Recargar,
    /// Marca de autoarranque.
    FijarAutoarranque(String, bool),
    /// Muestra un host externo en la lista (§3.1).
    Importar(String),
    /// Nuevo orden de la lista.
    Reordenar(Vec<String>),
    /// Crea o actualiza un host del fichero propio.
    GuardarHost(Box<Host>),
    /// Renombra un host y guarda el resto de sus cambios.
    RenombrarHost {
        antiguo: String,
        nuevo: Box<Host>,
    },
    /// Borra un host del fichero propio, bajando antes sus túneles.
    EliminarHost(String),
    Terminar,
}

/// Lo que circula por el canal del supervisor.
///
/// Las órdenes vienen de la interfaz; el resto lo mandan los hilos obreros.
/// Compartir canal es lo que permite que el bucle espere una sola cosa.
enum Mensaje {
    Orden(Orden),
    /// Un obrero terminó de conectar un túnel.
    Conectado {
        id: String,
        conexion: Conexion,
    },
    /// Una sonda de salud terminó.
    Sondeado {
        id: String,
        veredicto: Veredicto,
    },
}

/// La conexión en curso de un alias, con los túneles que esperan su maestro.
///
/// Un obrero por alias: dos aperturas simultáneas del mismo maestro se
/// pisarían el socket de control. Los túneles que llegan mientras tanto se
/// apuntan y salen en cuanto el maestro queda abierto, que además es el caso
/// rápido: sobre un maestro ya autenticado, activar un reenvío es instantáneo.
struct VueloAlias {
    conectando: String,
    pendientes: Vec<String>,
}

/// Contabilidad de un «levantar varios»: el resumen se da cuando termina el
/// último, porque con obreros ya no hay un final común que esperar en línea.
struct Lote {
    pendientes: HashSet<String>,
    total: usize,
    fallos: Vec<String>,
}

/// Lo que la interfaz lee en cada fotograma. Nunca bloquea al supervisor.
#[derive(Debug, Clone, Default)]
pub struct Instantanea {
    pub tuneles: Vec<Tunel>,
    pub hosts: Vec<Host>,
    pub estados: Vec<EstadoTunel>,
    pub importables: Vec<Host>,
    /// Identificadores marcados para levantarse al abrir la ventana.
    pub autoarranque: Vec<String>,
    /// Aviso puntual para mostrar en la interfaz (resultado de una orden).
    pub aviso: Option<Aviso>,
    /// Sube en cada cambio; sirve para saber si hay que repintar.
    pub generacion: u64,
}

impl Instantanea {
    pub fn estado(&self, id: &str) -> Option<&EstadoTunel> {
        self.estados.iter().find(|e| e.id == id)
    }

    pub fn cuenta(&self, estado: Estado) -> usize {
        self.estados.iter().filter(|e| e.estado == estado).count()
    }
}

/// Mensaje puntual para la interfaz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aviso {
    pub texto: String,
    pub grave: bool,
}

impl Aviso {
    pub fn informativo(texto: impl Into<String>) -> Aviso {
        Aviso {
            texto: texto.into(),
            grave: false,
        }
    }

    pub fn error(texto: impl Into<String>) -> Aviso {
        Aviso {
            texto: texto.into(),
            grave: true,
        }
    }
}

/// Función que la interfaz registra para enterarse de que hay novedades.
///
/// En la GUI es `ctx.request_repaint()`. Sin ella habría que repintar a sesenta
/// fotogramas por segundo para no perderse un cambio de estado, que es tirar
/// batería para no enseñar nada nuevo el 99 % de las veces.
type Despertador = Arc<Mutex<Option<Box<dyn Fn() + Send + 'static>>>>;

/// Asa del supervisor. Se queda en el hilo de la interfaz.
pub struct Supervisor {
    ordenes: Sender<Mensaje>,
    compartido: Arc<Mutex<Instantanea>>,
    hilo: Option<JoinHandle<()>>,
    /// Se despierta al hilo del dibujo cuando cambia algo.
    despertador: Despertador,
}

impl Supervisor {
    /// Arranca el supervisor sobre un motor ya construido.
    pub fn arrancar(motor: Motor, politica: PoliticaReintento) -> Supervisor {
        let (emisor, receptor) = mpsc::channel();
        let compartido = Arc::new(Mutex::new(Instantanea::default()));
        let despertador: Despertador = Arc::new(Mutex::new(None));

        let compartido_hilo = Arc::clone(&compartido);
        let despertador_hilo = Arc::clone(&despertador);
        let emisor_hilo = emisor.clone();
        let hilo = std::thread::Builder::new()
            .name("supervisor".to_string())
            .spawn(move || {
                let mut bucle = Bucle {
                    motor,
                    politica,
                    reparaciones: HashMap::new(),
                    compartido: compartido_hilo,
                    despertador: despertador_hilo,
                    generacion: 0,
                    emisor: emisor_hilo,
                    en_vuelo: HashMap::new(),
                    en_sonda: HashSet::new(),
                    sondas_viciadas: HashSet::new(),
                    lote: None,
                };
                bucle.ejecutar(receptor);
            })
            .expect("no se pudo crear el hilo del supervisor");

        Supervisor {
            ordenes: emisor,
            compartido,
            hilo: Some(hilo),
            despertador,
        }
    }

    /// Registra una función que se llama cuando hay novedades.
    ///
    /// En la GUI es `ctx.request_repaint()`: sin esto habría que repintar a 60
    /// fotogramas por segundo para no perderse nada, que es tirar batería.
    pub fn al_cambiar(&self, despertador: impl Fn() + Send + 'static) {
        if let Ok(mut ranura) = self.despertador.lock() {
            *ranura = Some(Box::new(despertador));
        }
    }

    pub fn enviar(&self, orden: Orden) {
        if self.ordenes.send(Mensaje::Orden(orden)).is_err() {
            tracing::warn!("el supervisor ya no escucha órdenes");
        }
    }

    pub fn instantanea(&self) -> Instantanea {
        self.compartido
            .lock()
            .map(|guarda| guarda.clone())
            .unwrap_or_default()
    }

    /// Consume el aviso pendiente, si lo hay.
    pub fn tomar_aviso(&self) -> Option<Aviso> {
        self.compartido
            .lock()
            .ok()
            .and_then(|mut guarda| guarda.aviso.take())
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.ordenes.send(Mensaje::Orden(Orden::Terminar));
        if let Some(hilo) = self.hilo.take() {
            let _ = hilo.join();
        }
    }
}

/// Estado interno del hilo supervisor.
struct Bucle {
    motor: Motor,
    politica: PoliticaReintento,
    /// Reparaciones suaves fallidas seguidas, por túnel.
    reparaciones: HashMap<String, u32>,
    compartido: Arc<Mutex<Instantanea>>,
    despertador: Despertador,
    generacion: u64,
    /// Por donde los hilos obreros devuelven su trabajo.
    emisor: Sender<Mensaje>,
    /// Conexiones en curso, una por alias.
    en_vuelo: HashMap<String, VueloAlias>,
    /// Sondas de salud en curso, por túnel.
    en_sonda: HashSet<String>,
    /// Sondas cuyo resultado ya no vale: el túnel se reconectó mientras
    /// volaban y el veredicto describiría la conexión anterior.
    sondas_viciadas: HashSet<String>,
    /// El «levantar varios» en curso, si lo hay.
    lote: Option<Lote>,
}

impl Bucle {
    fn ejecutar(&mut self, receptor: Receiver<Mensaje>) {
        if let Err(e) = self.motor.limpiar_al_arrancar() {
            tracing::warn!("la limpieza inicial de sockets falló: {e}");
        }
        self.publicar(None);

        // El latido se mide aparte del canal: los mensajes de los obreros
        // pueden llegar seguidos y `recv_timeout` a plazo fijo no vencería
        // nunca, dejando la supervisión sin ejecutar mientras haya tráfico.
        let mut ultimo_latido = Instant::now();
        loop {
            let restante = LATIDO.saturating_sub(ultimo_latido.elapsed());
            match receptor.recv_timeout(restante) {
                Ok(Mensaje::Orden(Orden::Terminar)) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(mensaje) => {
                    let aviso = self.atender_mensaje(mensaje);
                    self.observar_y_sondear();
                    self.publicar(aviso);
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.observar_y_sondear();
                    self.supervisar();
                    self.publicar(None);
                    ultimo_latido = Instant::now();
                }
            }
        }
        tracing::info!("supervisor detenido; los maestros siguen en pie");
    }

    fn atender_mensaje(&mut self, mensaje: Mensaje) -> Option<Aviso> {
        match mensaje {
            Mensaje::Orden(orden) => self.atender(orden),
            Mensaje::Conectado { id, conexion } => self.concluir_conexion(&id, conexion),
            Mensaje::Sondeado { id, veredicto } => {
                self.en_sonda.remove(&id);
                if self.sondas_viciadas.remove(&id) {
                    // El túnel se reconectó mientras la sonda volaba: el
                    // veredicto habla de una conexión que ya no existe.
                    return None;
                }
                self.motor.fijar_salud(&id, veredicto);
                None
            }
        }
    }

    /// Observa la realidad y manda a hilos obreros las sondas que toquen.
    ///
    /// Las sondas atraviesan el túnel y tardan hasta cuatro segundos: dentro
    /// del bucle congelarían la atención de órdenes, que es justo lo que el
    /// latido debe evitar.
    fn observar_y_sondear(&mut self) {
        for sonda in self.motor.observar_sin_sondear() {
            if self.en_sonda.contains(&sonda.id) {
                continue;
            }
            let id = sonda.id.clone();
            self.en_sonda.insert(id.clone());
            let emisor = self.emisor.clone();
            let lanzado = std::thread::Builder::new()
                .name(format!("sonda-{}", sonda.id))
                .spawn(move || {
                    let veredicto = salud::sondear(
                        &sonda.salud,
                        &sonda.direccion,
                        sonda.puerto,
                        sonda.escuchando,
                    );
                    let _ = emisor.send(Mensaje::Sondeado {
                        id: sonda.id,
                        veredicto,
                    });
                });
            if lanzado.is_err() {
                // Sin hilo no hay sonda esta ronda; la siguiente lo reintenta.
                self.en_sonda.remove(&id);
            }
        }
    }

    fn atender(&mut self, orden: Orden) -> Option<Aviso> {
        match orden {
            Orden::Levantar(id) => self.levantar(&id),
            Orden::Bajar(id) => self.bajar(&id),
            Orden::LevantarVarios(ids) => {
                match &mut self.lote {
                    Some(lote) => {
                        for id in &ids {
                            if lote.pendientes.insert(id.clone()) {
                                lote.total += 1;
                            }
                        }
                    }
                    None => {
                        self.lote = Some(Lote {
                            pendientes: ids.iter().cloned().collect(),
                            total: ids.len(),
                            fallos: Vec::new(),
                        });
                    }
                }
                let cuantos = ids.len();
                let mut resumen = None;
                for id in &ids {
                    // Un fallo de preparación concluye el túnel aquí mismo, y
                    // con él puede concluir el lote entero.
                    if let Some(aviso) = self.levantar(id) {
                        resumen.get_or_insert(aviso);
                    }
                }
                resumen
                    .or_else(|| Some(Aviso::informativo(format!("levantando {cuantos} túneles…"))))
            }
            Orden::BajarVarios(ids) => {
                for id in &ids {
                    self.bajar(id);
                }
                Some(Aviso::informativo(format!("{} túneles bajados", ids.len())))
            }
            Orden::ReintentarYa(id) => {
                if let Some(estado) = self.motor.estados().into_iter().find(|e| e.id == id) {
                    if estado.estado == Estado::Fallido {
                        // Salir de Fallido a mano reinicia el contador.
                        self.reiniciar_contador(&id);
                    }
                }
                self.levantar(&id)
            }
            Orden::Recargar => match self.motor.recargar() {
                Ok(()) => Some(Aviso::informativo("configuración recargada")),
                Err(e) => Some(Aviso::error(mensaje_completo(&e))),
            },
            Orden::FijarAutoarranque(id, valor) => {
                match self.motor.base.fijar_autoarranque(&id, valor) {
                    Ok(()) => None,
                    Err(e) => Some(Aviso::error(mensaje_completo(&e))),
                }
            }
            Orden::Importar(alias) => match self.motor.importar(&alias) {
                Ok(()) => Some(Aviso::informativo(format!("«{alias}» añadido a la lista"))),
                Err(e) => Some(Aviso::error(mensaje_completo(&e))),
            },
            Orden::Reordenar(ids) => match self.motor.base.fijar_orden(&ids) {
                Ok(()) => None,
                Err(e) => Some(Aviso::error(mensaje_completo(&e))),
            },
            Orden::GuardarHost(host) => self.guardar_host(&host),
            Orden::RenombrarHost { antiguo, nuevo } => {
                // Los túneles del alias viejo dejan de existir: se bajan antes
                // de tocar el fichero para no dejar maestros sin dueño.
                self.bajar_los_de(&antiguo);
                match self.motor.catalogo.renombrar_host(&antiguo, &nuevo.alias) {
                    Ok(()) => self.guardar_host(&nuevo),
                    Err(e) => Some(Aviso::error(mensaje_completo(&e))),
                }
            }
            Orden::EliminarHost(alias) => {
                self.bajar_los_de(&alias);
                match self.motor.catalogo.eliminar_host(&alias) {
                    Ok(()) => match self.motor.recargar() {
                        Ok(()) => Some(Aviso::informativo(format!("«{alias}» eliminado"))),
                        Err(e) => Some(Aviso::error(mensaje_completo(&e))),
                    },
                    Err(e) => Some(Aviso::error(mensaje_completo(&e))),
                }
            }
            Orden::Terminar => None,
        }
    }

    fn guardar_host(&mut self, host: &Host) -> Option<Aviso> {
        let alias = host.alias.clone();
        match self.motor.catalogo.guardar_host(host) {
            Ok(()) => match self.motor.recargar() {
                Ok(()) => Some(Aviso::informativo(format!("«{alias}» guardado"))),
                Err(e) => Some(Aviso::error(mensaje_completo(&e))),
            },
            Err(e) => Some(Aviso::error(mensaje_completo(&e))),
        }
    }

    /// Baja todos los túneles de un alias antes de modificarlo o borrarlo.
    fn bajar_los_de(&mut self, alias: &str) {
        let ids: Vec<String> = self
            .motor
            .catalogo
            .tuneles()
            .iter()
            .filter(|t| t.alias == alias)
            .map(|t| t.id())
            .collect();
        for id in ids {
            if self.motor.estado(&id).estado.deberia_estar_arriba() {
                let _ = self.motor.bajar(&id);
            }
        }
    }

    /// Pone un túnel a levantarse sin esperar a que termine.
    ///
    /// La fase previa —validar, anotar la intención, pasar a `Conectando`— se
    /// hace aquí, en el bucle; la conexión, que puede tardar dos minutos de
    /// autenticación, se va a un hilo obrero y vuelve como [`Mensaje::Conectado`].
    fn levantar(&mut self, id: &str) -> Option<Aviso> {
        let plan = match self.motor.preparar_levantar(id) {
            Ok(plan) => plan,
            Err(e) => {
                self.programar_reintento(id);
                let texto = format!("{id}: {}", mensaje_completo(&e));
                let (en_lote, resumen) = self.anotar_en_lote(id, Some(&texto));
                return resumen.or(if en_lote {
                    None
                } else {
                    Some(Aviso::error(texto))
                });
            }
        };

        // Si hay una sonda en vuelo, su veredicto describirá la conexión que
        // esta reconexión sustituye: se descarta cuando llegue.
        if self.en_sonda.contains(id) {
            self.sondas_viciadas.insert(id.to_string());
        }

        let alias = plan.tunel.alias.clone();
        if let Some(vuelo) = self.en_vuelo.get_mut(&alias) {
            // El maestro de este alias ya se está abriendo: a la cola. Saldrá
            // en cuanto abra, que es además el caso rápido.
            if vuelo.conectando != id && !vuelo.pendientes.iter().any(|p| p == id) {
                vuelo.pendientes.push(id.to_string());
            }
            return None;
        }

        self.lanzar_conexion(alias, id.to_string(), plan);
        None
    }

    fn lanzar_conexion(&mut self, alias: String, id: String, plan: PlanDeConexion) {
        let emisor = self.emisor.clone();
        let id_obrero = id.clone();
        let lanzado = std::thread::Builder::new()
            .name(format!("conexion-{alias}"))
            .spawn(move || {
                let conexion = conectar(&plan);
                let _ = emisor.send(Mensaje::Conectado {
                    id: id_obrero,
                    conexion,
                });
            });
        match lanzado {
            Ok(_) => {
                self.en_vuelo.insert(
                    alias,
                    VueloAlias {
                        conectando: id,
                        pendientes: Vec::new(),
                    },
                );
            }
            Err(e) => {
                // Sin hilo no hay conexión esta vez; el reintento programado
                // la vuelve a pedir en unos segundos.
                tracing::warn!(id = %id, "no se pudo crear el hilo de conexión: {e}");
                self.programar_reintento(&id);
            }
        }
    }

    /// Aplica el resultado que trae un obrero y despacha la cola de su alias.
    fn concluir_conexion(&mut self, id: &str, conexion: Conexion) -> Option<Aviso> {
        let alias = self
            .en_vuelo
            .iter()
            .find(|(_, vuelo)| vuelo.conectando == id)
            .map(|(alias, _)| alias.clone());
        let pendientes = match &alias {
            Some(alias) => self
                .en_vuelo
                .remove(alias)
                .map(|vuelo| vuelo.pendientes)
                .unwrap_or_default(),
            None => Vec::new(),
        };

        let mut aviso = if self.motor.tunel(id).is_ok() {
            match self.motor.concluir_levantar(id, conexion) {
                Ok(()) => self.anotar_en_lote(id, None).1,
                Err(e) => {
                    self.programar_reintento(id);
                    let texto = format!("{id}: {}", mensaje_completo(&e));
                    let (en_lote, resumen) = self.anotar_en_lote(id, Some(&texto));
                    resumen.or(if en_lote {
                        None
                    } else {
                        Some(Aviso::error(texto))
                    })
                }
            }
        } else {
            // El túnel se borró mientras el obrero conectaba. Si dejó un
            // maestro abierto que ya nadie del alias quiere, se cierra: una
            // carrera no puede dejar huérfanos.
            if conexion.resultado.is_ok() {
                if let Some(alias) = &alias {
                    if !self.motor.quedan_deseados_en(alias, id) {
                        if let Ok(maestro) = Maestro::de(alias) {
                            let _ = maestro.cerrar();
                        }
                    }
                }
            }
            self.anotar_en_lote(id, None).1
        };

        // Los que esperaban el maestro de este alias salen ahora.
        for pendiente in pendientes {
            if let Some(otro) = self.levantar(&pendiente) {
                aviso.get_or_insert(otro);
            }
        }
        aviso
    }

    /// Da un túnel por concluido dentro del lote, si pertenece a uno.
    ///
    /// Devuelve si pertenecía y, cuando era el último, el aviso de resumen.
    fn anotar_en_lote(&mut self, id: &str, fallo: Option<&str>) -> (bool, Option<Aviso>) {
        let Some(lote) = &mut self.lote else {
            return (false, None);
        };
        if !lote.pendientes.remove(id) {
            return (false, None);
        }
        if let Some(fallo) = fallo {
            lote.fallos.push(fallo.to_string());
        }
        if !lote.pendientes.is_empty() {
            return (true, None);
        }
        let lote = self.lote.take().expect("el lote existe: se acaba de usar");
        let resumen = if lote.fallos.is_empty() {
            Aviso::informativo(format!("{} túneles levantados", lote.total))
        } else {
            Aviso::error(lote.fallos.join("\n"))
        };
        (true, Some(resumen))
    }

    fn bajar(&mut self, id: &str) -> Option<Aviso> {
        match self.motor.bajar(id) {
            Ok(()) => None,
            Err(e) => Some(Aviso::error(format!("{id}: {}", mensaje_completo(&e)))),
        }
    }

    fn reiniciar_contador(&mut self, id: &str) {
        let mut estado = self.motor.estado(id);
        estado.intento = 0;
        estado.proximo_intento = None;
        self.motor.fijar_estado(estado);
    }

    /// Programa el siguiente intento con retardo exponencial y jitter.
    fn programar_reintento(&mut self, id: &str) {
        let mut estado = self.motor.estado(id);
        estado.intento += 1;

        if self.politica.agotado(estado.intento) {
            tracing::warn!(id = %id, intentos = estado.intento, "reintentos agotados");
            estado.proximo_intento = None;
            estado.estado = Estado::Fallido;
            estado.desde = Instant::now();
            self.motor.fijar_estado(estado);
            return;
        }

        let retardo = self.politica.retardo(estado.intento);
        tracing::info!(
            id = %id,
            intento = estado.intento,
            segundos = retardo.as_secs_f32(),
            "siguiente reintento programado"
        );
        estado.proximo_intento = Some(Instant::now() + retardo);
        estado.estado = Estado::Reintentando;
        estado.desde = Instant::now();
        self.motor.fijar_estado(estado);
    }

    /// Un latido de supervisión: recupera lo que toque recuperar.
    fn supervisar(&mut self) {
        let ahora = Instant::now();
        let pendientes: Vec<(String, Estado)> = self
            .motor
            .estados()
            .into_iter()
            .filter(|e| match e.estado {
                Estado::Reintentando => e.proximo_intento.map(|m| m <= ahora).unwrap_or(true),
                Estado::Degradado => e.proximo_intento.map(|m| m <= ahora).unwrap_or(true),
                _ => false,
            })
            .map(|e| (e.id, e.estado))
            .collect();

        for (id, estado) in pendientes {
            match estado {
                // Maestro vivo, reenvío caído o zombi. Se escala en dos
                // escalones: primero rehacer el reenvío, que es barato y no
                // reautentica; si insiste, reabrir el maestro entero.
                Estado::Degradado => {
                    let intentos = *self.reparaciones.get(&id).unwrap_or(&0);
                    if intentos < REPARACIONES_ANTES_DE_REABRIR {
                        match self.motor.reparar(&id) {
                            Ok(()) => {
                                self.reparaciones.remove(&id);
                                tracing::info!(id = %id, "túnel recuperado");
                            }
                            Err(e) => {
                                let cuenta = self.reparaciones.entry(id.clone()).or_insert(0);
                                *cuenta += 1;
                                tracing::warn!(
                                    id = %id,
                                    intento = *cuenta,
                                    "no se pudo reparar el túnel: {e}"
                                );
                                self.programar_reintento(&id);
                            }
                        }
                    } else {
                        // Reabrir el maestro es cerrar —rápido— y reconectar.
                        // La reconexión se va a los obreros como cualquier
                        // otra; si falla, cae en la maquinaria de reintentos.
                        match self.motor.preparar_reparacion_a_fondo(&id) {
                            Ok(deseados) => {
                                self.reparaciones.remove(&id);
                                for otro in deseados {
                                    let _ = self.levantar(&otro);
                                }
                            }
                            Err(e) => {
                                let cuenta = self.reparaciones.entry(id.clone()).or_insert(0);
                                *cuenta += 1;
                                tracing::warn!(
                                    id = %id,
                                    intento = *cuenta,
                                    "no se pudo reabrir la conexión maestra: {e}"
                                );
                                self.programar_reintento(&id);
                            }
                        }
                    }
                }
                // Sin maestro: reconexión completa, en un hilo obrero. El
                // fallo de la fase previa ya programa el siguiente reintento
                // dentro de «levantar»; el de la conexión llega como mensaje.
                Estado::Reintentando => {
                    let _ = self.levantar(&id);
                }
                _ => {}
            }
        }
    }

    fn publicar(&mut self, aviso: Option<Aviso>) {
        self.generacion += 1;
        let nueva = Instantanea {
            tuneles: self.motor.tuneles_visibles(),
            hosts: self.motor.catalogo.todos().into_iter().cloned().collect(),
            estados: self.motor.estados(),
            importables: self.motor.importables().into_iter().cloned().collect(),
            autoarranque: self.motor.de_autoarranque(),
            aviso,
            generacion: self.generacion,
        };

        if let Ok(mut guarda) = self.compartido.lock() {
            // Un aviso todavía sin leer no se pisa.
            let aviso_previo = guarda.aviso.take();
            *guarda = nueva;
            if guarda.aviso.is_none() {
                guarda.aviso = aviso_previo;
            }
        }

        if let Ok(guarda) = self.despertador.lock() {
            if let Some(despertar) = guarda.as_ref() {
                despertar();
            }
        }
    }
}

/// Levanta los túneles marcados de autoarranque.
pub fn arrancar_automaticos(supervisor: &Supervisor, ids: &[String]) -> Resultado<()> {
    if ids.is_empty() {
        return Ok(());
    }
    tracing::info!(cantidad = ids.len(), "levantando túneles de autoarranque");
    supervisor.enviar(Orden::LevantarVarios(ids.to_vec()));
    Ok(())
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn el_retardo_crece_de_forma_exponencial() {
        let politica = PoliticaReintento {
            jitter: 0.0,
            ..Default::default()
        };
        assert_eq!(politica.retardo(1), Duration::from_secs(2));
        assert_eq!(politica.retardo(2), Duration::from_secs(4));
        assert_eq!(politica.retardo(3), Duration::from_secs(8));
        assert_eq!(politica.retardo(4), Duration::from_secs(16));
        assert_eq!(politica.retardo(5), Duration::from_secs(32));
    }

    #[test]
    fn el_retardo_se_detiene_en_el_techo() {
        let politica = PoliticaReintento {
            jitter: 0.0,
            ..Default::default()
        };
        assert_eq!(politica.retardo(6), Duration::from_secs(60));
        assert_eq!(politica.retardo(30), Duration::from_secs(60));
        // Sin acotar el exponente esto desbordaría.
        assert_eq!(politica.retardo(u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn el_jitter_se_queda_dentro_del_margen() {
        let politica = PoliticaReintento::default();
        for _ in 0..200 {
            let retardo = politica.retardo(6).as_secs_f64();
            assert!(
                (48.0..=72.0).contains(&retardo),
                "el jitter del ±20 % sobre 60 s se salió: {retardo}"
            );
        }
    }

    #[test]
    fn el_jitter_reparte_de_verdad() {
        // Si todos los reintentos cayeran en el mismo instante, veinte túneles
        // que se caen con la VPN volverían a la vez y tumbarían el bastión.
        let politica = PoliticaReintento::default();
        let muestras: Vec<u128> = (0..50).map(|_| politica.retardo(4).as_millis()).collect();
        let distintos: std::collections::HashSet<_> = muestras.iter().collect();
        assert!(distintos.len() > 40, "el jitter apenas dispersa");
    }

    #[test]
    fn los_intentos_se_agotan() {
        let politica = PoliticaReintento {
            maximo_intentos: 3,
            ..Default::default()
        };
        assert!(!politica.agotado(2));
        assert!(politica.agotado(3));
        assert!(politica.agotado(4));
    }

    #[test]
    fn sin_limite_nunca_se_agota() {
        let politica = PoliticaReintento {
            maximo_intentos: 0,
            ..Default::default()
        };
        assert!(!politica.agotado(1_000_000));
    }
}
