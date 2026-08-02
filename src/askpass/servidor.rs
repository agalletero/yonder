//! Servidor del askpass: vive en la aplicación (§5.1).
//!
//! Escucha en `$XDG_RUNTIME_DIR/yonder/askpass.sock`. Cada vez que `ssh`
//! necesita una contraseña, lanza `yonder-askpass`, este se conecta, y la
//! petición aparece en la cola. La interfaz la recoge, enseña el modal y
//! responde.
//!
//! Nada de esto toca el disco: la respuesta va del teclado al socket y de ahí a
//! la entrada estándar de `ssh`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::{Error, Resultado};
use crate::rutas;

use super::protocolo::{self, Respuesta, Tipo};

/// Una petición pendiente de respuesta, tal como la ve la interfaz.
#[derive(Debug)]
pub struct PeticionPendiente {
    pub id: u64,
    pub prompt: String,
    pub tipo: Tipo,
    pub pid: u32,
    respondedor: Sender<Respuesta>,
}

impl PeticionPendiente {
    /// Envía la respuesta al proceso `ssh` que espera.
    pub fn responder(self, texto: impl Into<String>) {
        let _ = self.respondedor.send(Respuesta::con(texto));
    }

    /// El usuario ha cancelado: `ssh` fallará la autenticación, que es lo
    /// correcto.
    pub fn cancelar(self) {
        let _ = self.respondedor.send(Respuesta::cancelada());
    }

    /// Texto limpio para el modal: `ssh` acaba los prompts con «: ».
    pub fn texto_visible(&self) -> String {
        self.prompt.trim().trim_end_matches(':').trim().to_string()
    }
}

/// Función que la interfaz registra para enterarse de que ha llegado una
/// petición.
///
/// En la GUI es `ctx.request_repaint()`. Va tras un `Mutex` porque se registra
/// desde el hilo del dibujado y se invoca desde el del socket, y tras un
/// `Option` porque el servidor arranca antes de que haya ventana a la que
/// avisar.
type Despertador = Arc<Mutex<Option<Box<dyn Fn() + Send + 'static>>>>;

/// Servidor del askpass.
pub struct Servidor {
    socket: PathBuf,
    cola: Arc<Mutex<VecDeque<PeticionPendiente>>>,
    parar: Arc<AtomicBool>,
    despertador: Despertador,
}

impl std::fmt::Debug for Servidor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Servidor")
            .field("socket", &self.socket)
            .finish_non_exhaustive()
    }
}

impl Servidor {
    /// Arranca el servidor en la ruta de §6.
    pub fn arrancar() -> Resultado<Servidor> {
        Servidor::arrancar_en(&rutas::socket_askpass()?)
    }

    pub fn arrancar_en(socket: &Path) -> Resultado<Servidor> {
        if let Some(directorio) = socket.parent() {
            rutas::asegurar_directorio_privado(directorio)?;
        }
        // Un socket de una ejecución anterior impediría el bind.
        let _ = std::fs::remove_file(socket);

        let escucha = UnixListener::bind(socket).map_err(|e| Error::escribiendo(socket, e))?;
        // El directorio ya es 0700, así que nadie más llega aquí; el 0600 del
        // socket es la segunda vuelta de llave.
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::escribiendo(socket, e))?;

        let cola = Arc::new(Mutex::new(VecDeque::new()));
        let parar = Arc::new(AtomicBool::new(false));
        let despertador: Despertador = Arc::new(Mutex::new(None));

        let cola_hilo = Arc::clone(&cola);
        let parar_hilo = Arc::clone(&parar);
        let despertador_hilo = Arc::clone(&despertador);
        let socket_hilo = socket.to_path_buf();

        std::thread::Builder::new()
            .name("askpass".to_string())
            .spawn(move || {
                atender(escucha, cola_hilo, parar_hilo, despertador_hilo);
                let _ = std::fs::remove_file(&socket_hilo);
            })
            .map_err(Error::Io)?;

        tracing::info!(socket = %rutas::abreviar(socket), "servidor del askpass en marcha");
        Ok(Servidor {
            socket: socket.to_path_buf(),
            cola,
            parar,
            despertador,
        })
    }

    /// Ruta del socket, para pasarla en `SSH_ASKPASS`.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Registra la función que despierta a la interfaz cuando llega una petición.
    pub fn al_llegar(&self, despertador: impl Fn() + Send + 'static) {
        if let Ok(mut ranura) = self.despertador.lock() {
            *ranura = Some(Box::new(despertador));
        }
    }

    /// Saca la siguiente petición pendiente, si la hay.
    pub fn siguiente(&self) -> Option<PeticionPendiente> {
        self.cola.lock().ok().and_then(|mut cola| cola.pop_front())
    }

    pub fn pendientes(&self) -> usize {
        self.cola.lock().map(|cola| cola.len()).unwrap_or(0)
    }
}

impl Drop for Servidor {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::SeqCst);
        // Una conexión de cortesía desbloquea el `accept` para que el hilo vea
        // la bandera y salga.
        let _ = UnixStream::connect(&self.socket);
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn atender(
    escucha: UnixListener,
    cola: Arc<Mutex<VecDeque<PeticionPendiente>>>,
    parar: Arc<AtomicBool>,
    despertador: Despertador,
) {
    static SIGUIENTE_ID: AtomicU64 = AtomicU64::new(1);

    for conexion in escucha.incoming() {
        if parar.load(Ordering::SeqCst) {
            break;
        }
        let flujo = match conexion {
            Ok(flujo) => flujo,
            Err(e) => {
                tracing::warn!("conexión del askpass rechazada: {e}");
                continue;
            }
        };

        let id = SIGUIENTE_ID.fetch_add(1, Ordering::SeqCst);
        let cola_conexion = Arc::clone(&cola);
        let despertador_conexion = Arc::clone(&despertador);

        // Un hilo por conexión: el manejador se queda bloqueado esperando a que
        // el usuario conteste, y mientras tanto pueden llegar otras.
        std::thread::spawn(move || {
            if let Err(e) = dialogar(flujo, id, cola_conexion, despertador_conexion) {
                tracing::warn!(id, "el diálogo con el askpass falló: {e}");
            }
        });
    }
}

fn dialogar(
    mut flujo: UnixStream,
    id: u64,
    cola: Arc<Mutex<VecDeque<PeticionPendiente>>>,
    despertador: Despertador,
) -> Resultado<()> {
    flujo
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(Error::Io)?;

    let mut lector = BufReader::new(flujo.try_clone().map_err(Error::Io)?);
    let mut linea = String::new();
    lector.read_line(&mut linea).map_err(Error::Io)?;

    let peticion: protocolo::Peticion = serde_json::from_str(linea.trim())
        .map_err(|e| Error::DefinicionInvalida(format!("petición del askpass ilegible: {e}")))?;

    if peticion.version != protocolo::VERSION {
        return Err(Error::DefinicionInvalida(format!(
            "versión de protocolo {} no admitida",
            peticion.version
        )));
    }

    tracing::info!(id, tipo = ?peticion.tipo, "el askpass pide una respuesta");

    let (emisor, receptor) = mpsc::channel();
    if let Ok(mut cola) = cola.lock() {
        cola.push_back(PeticionPendiente {
            id,
            prompt: peticion.prompt,
            tipo: peticion.tipo,
            pid: peticion.pid,
            respondedor: emisor,
        });
    }
    if let Ok(guarda) = despertador.lock() {
        if let Some(despertar) = guarda.as_ref() {
            despertar();
        }
    }

    // Si nadie contesta, cancelar es mejor que dejar a `ssh` colgado.
    let respuesta = receptor
        .recv_timeout(Duration::from_secs(protocolo::ESPERA_RESPUESTA_SEGUNDOS))
        .unwrap_or_else(|_| {
            tracing::warn!(id, "nadie respondió al askpass: se cancela");
            Respuesta::cancelada()
        });

    let serializada =
        serde_json::to_string(&respuesta).map_err(|e| Error::DefinicionInvalida(e.to_string()))?;
    writeln!(flujo, "{serializada}").map_err(Error::Io)?;
    flujo.flush().map_err(Error::Io)?;
    Ok(())
}

/// Localiza el binario `yonder-askpass`.
///
/// Primero al lado del ejecutable actual, que es como queda tras un `cargo
/// build` y tras descomprimir la *release*. Después, por el PATH.
pub fn localizar_binario() -> Option<PathBuf> {
    if let Ok(actual) = std::env::current_exe() {
        if let Some(directorio) = actual.parent() {
            let candidato = directorio.join("yonder-askpass");
            if candidato.is_file() {
                return Some(candidato);
            }
        }
    }
    std::env::var_os("PATH")
        .map(|ruta| std::env::split_paths(&ruta).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|directorio| directorio.join("yonder-askpass"))
        .find(|candidato| candidato.is_file())
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use crate::askpass::cliente;

    #[test]
    fn una_ronda_completa_de_peticion_y_respuesta() {
        let directorio = tempfile::tempdir().unwrap();
        let socket = directorio.path().join("askpass.sock");
        let servidor = Servidor::arrancar_en(&socket).unwrap();

        let socket_cliente = socket.clone();
        let cliente = std::thread::spawn(move || {
            cliente::preguntar(&socket_cliente, "usuario@host's password: ")
        });

        // La petición debe aparecer en la cola.
        let mut pendiente = None;
        for _ in 0..100 {
            if let Some(p) = servidor.siguiente() {
                pendiente = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let pendiente = pendiente.expect("la petición no llegó a la cola");
        assert_eq!(pendiente.tipo, Tipo::Contrasena);
        assert_eq!(pendiente.texto_visible(), "usuario@host's password");
        pendiente.responder("secreta");

        let respuesta = cliente.join().unwrap().unwrap();
        assert_eq!(respuesta.texto.as_deref(), Some("secreta"));
    }

    #[test]
    fn la_cancelacion_llega_al_cliente() {
        let directorio = tempfile::tempdir().unwrap();
        let socket = directorio.path().join("askpass.sock");
        let servidor = Servidor::arrancar_en(&socket).unwrap();

        let socket_cliente = socket.clone();
        let cliente = std::thread::spawn(move || {
            cliente::preguntar(&socket_cliente, "Enter passphrase for key '/x': ")
        });

        let mut pendiente = None;
        for _ in 0..100 {
            if let Some(p) = servidor.siguiente() {
                pendiente = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let pendiente = pendiente.expect("la petición no llegó a la cola");
        assert_eq!(pendiente.tipo, Tipo::Passphrase);
        pendiente.cancelar();

        let respuesta = cliente.join().unwrap().unwrap();
        assert!(respuesta.texto.is_none());
    }

    #[test]
    fn el_socket_es_privado() {
        let directorio = tempfile::tempdir().unwrap();
        let socket = directorio.path().join("askpass.sock");
        let _servidor = Servidor::arrancar_en(&socket).unwrap();
        let permisos = std::fs::metadata(&socket).unwrap().permissions();
        assert_eq!(permisos.mode() & 0o777, 0o600);
    }

    #[test]
    fn atiende_varias_peticiones_a_la_vez() {
        let directorio = tempfile::tempdir().unwrap();
        let socket = directorio.path().join("askpass.sock");
        let servidor = Servidor::arrancar_en(&socket).unwrap();

        let clientes: Vec<_> = (0..3)
            .map(|indice| {
                let socket = socket.clone();
                std::thread::spawn(move || {
                    cliente::preguntar(&socket, &format!("host{indice}'s password: "))
                })
            })
            .collect();

        let mut atendidas = 0;
        for _ in 0..200 {
            while let Some(pendiente) = servidor.siguiente() {
                pendiente.responder("x");
                atendidas += 1;
            }
            if atendidas == 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(atendidas, 3);
        for cliente in clientes {
            assert_eq!(cliente.join().unwrap().unwrap().texto.as_deref(), Some("x"));
        }
    }
}
