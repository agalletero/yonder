//! Sondeo de puertos locales (§5.3 y §5.4).
//!
//! Dos usos distintos y ambos imprescindibles:
//!
//! - **Antes de conectar**: comprobar que el puerto local está libre y, si no
//!   lo está, decir *quién* lo ocupa. Sin esto `ssh` falla con un mensaje que
//!   no ayuda a nadie (criterio de aceptación 5, §12).
//! - **Mientras el túnel está activo**: comprobar que el puerto sigue en
//!   escucha. Es lo que distingue `Activo` de `Degradado` (§4). Se hace
//!   leyendo `/proc/net/tcp`, no conectando: abrir una conexión al puerto
//!   atravesaría el túnel y tendría efectos en el otro extremo.
//!
//! Todo se resuelve con `/proc`, sin depender de `lsof`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, ToSocketAddrs};
use std::path::Path;

use crate::error::{Error, Resultado};

/// Estado TCP «LISTEN» tal como lo escribe el núcleo en `/proc/net/tcp`.
const ESTADO_LISTEN: &str = "0A";

/// Proceso que ocupa un puerto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ocupante {
    pub pid: Option<u32>,
    /// Nombre corto del proceso (`/proc/PID/comm`).
    pub nombre: Option<String>,
    /// Usuario propietario del socket, si se pudo determinar.
    pub uid: Option<u32>,
}

impl std::fmt::Display for Ocupante {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.nombre, self.pid) {
            (Some(nombre), Some(pid)) => write!(f, "«{nombre}» (PID {pid})"),
            (Some(nombre), None) => write!(f, "«{nombre}»"),
            (None, Some(pid)) => write!(f, "el proceso {pid}"),
            (None, None) => match self.uid {
                Some(uid) => write!(f, "un proceso del usuario {uid}"),
                None => write!(f, "otro proceso"),
            },
        }
    }
}

/// Una entrada de `/proc/net/tcp` o `/proc/net/tcp6`.
#[derive(Debug, Clone)]
struct Socket {
    direccion: IpAddr,
    puerto: u16,
    estado: String,
    uid: u32,
    inodo: u64,
}

/// Comprueba que se puede abrir el puerto local antes de pedírselo a `ssh`.
///
/// Devuelve un error explícito con el ocupante si está tomado, y otro distinto
/// si es un puerto privilegiado sin la capacidad necesaria.
pub fn comprobar_puerto_libre(direccion: &str, puerto: u16) -> Resultado<()> {
    if puerto < 1024 && !puede_abrir_puertos_privilegiados() {
        return Err(Error::PuertoPrivilegiado { puerto });
    }

    let destinos: Vec<SocketAddr> = (direccion, puerto)
        .to_socket_addrs()
        .map_err(|e| Error::DefinicionInvalida(format!("«{direccion}» no se pudo resolver: {e}")))?
        .collect();

    if destinos.is_empty() {
        return Err(Error::DefinicionInvalida(format!(
            "«{direccion}» no resuelve a ninguna dirección"
        )));
    }

    for destino in destinos {
        match TcpListener::bind(destino) {
            Ok(escucha) => drop(escucha),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(Error::PuertoOcupado {
                    puerto,
                    ocupante: quien_ocupa(puerto).map(|o| o.to_string()),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(Error::PuertoPrivilegiado { puerto });
            }
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

/// Fotografía de todos los puertos en escucha en un instante.
///
/// El supervisor comprueba muchos túneles por cada latido. Leer `/proc/net/tcp`
/// una vez y consultarla N veces cuesta una lectura; hacerlo al revés cuesta N.
#[derive(Debug, Clone, Default)]
pub struct Escuchas {
    entradas: Vec<(IpAddr, u16)>,
}

impl Escuchas {
    pub fn tomar() -> Escuchas {
        Escuchas {
            entradas: sockets()
                .into_iter()
                .filter(|s| s.estado == ESTADO_LISTEN)
                .map(|s| (s.direccion, s.puerto))
                .collect(),
        }
    }

    /// `true` si hay un socket en escucha que cubra esa dirección y puerto.
    ///
    /// Un socket comodín (`0.0.0.0` o `::`) cubre cualquier dirección local,
    /// así que también cuenta.
    pub fn contiene(&self, direccion: &str, puerto: u16) -> bool {
        let objetivo: Option<IpAddr> = direccion.parse().ok();
        self.entradas
            .iter()
            .filter(|(_, p)| *p == puerto)
            .any(|(ip, _)| match objetivo {
                Some(buscada) => *ip == buscada || es_comodin(ip),
                // Si la dirección de escucha es un nombre y no una IP, basta
                // con que algo escuche en ese puerto.
                None => true,
            })
    }
}

/// Atajo de un solo uso sobre [`Escuchas`].
pub fn esta_escuchando(direccion: &str, puerto: u16) -> bool {
    Escuchas::tomar().contiene(direccion, puerto)
}

fn es_comodin(direccion: &IpAddr) -> bool {
    match direccion {
        IpAddr::V4(v4) => v4.is_unspecified(),
        IpAddr::V6(v6) => v6.is_unspecified(),
    }
}

/// Identifica el proceso que tiene un puerto en escucha.
pub fn quien_ocupa(puerto: u16) -> Option<Ocupante> {
    let socket = sockets()
        .into_iter()
        .find(|s| s.puerto == puerto && s.estado == ESTADO_LISTEN)?;

    let (pid, nombre) = match proceso_de_inodo(socket.inodo) {
        Some((pid, nombre)) => (Some(pid), nombre),
        None => (None, None),
    };

    Some(Ocupante {
        pid,
        nombre,
        uid: Some(socket.uid),
    })
}

/// `true` si el proceso puede abrir puertos por debajo de 1024.
///
/// Es cierto si es root o si tiene `CAP_NET_BIND_SERVICE` en su conjunto
/// efectivo. Detectarlo permite avisar en la interfaz **antes** de intentar la
/// conexión (§5.4).
pub fn puede_abrir_puertos_privilegiados() -> bool {
    if unsafe { libc::geteuid() } == 0 {
        return true;
    }
    const CAP_NET_BIND_SERVICE: u64 = 10;
    capacidades_efectivas()
        .map(|caps| caps & (1 << CAP_NET_BIND_SERVICE) != 0)
        .unwrap_or(false)
}

fn capacidades_efectivas() -> Option<u64> {
    let estado = std::fs::read_to_string("/proc/self/status").ok()?;
    let linea = estado.lines().find(|l| l.starts_with("CapEff:"))?;
    let hexadecimal = linea.split_whitespace().nth(1)?;
    u64::from_str_radix(hexadecimal, 16).ok()
}

/// Orden que hay que ejecutar para conceder la capacidad (§5.4).
pub fn orden_para_conceder_capacidad() -> String {
    let binario = std::env::current_exe()
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| "<PATH>/yonder".to_string());
    format!("sudo setcap 'cap_net_bind_service=+ep' {binario}")
}

/// Lee `/proc/net/tcp` y `/proc/net/tcp6`.
fn sockets() -> Vec<Socket> {
    let mut salida = Vec::new();
    for (ruta, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let contenido = match std::fs::read_to_string(ruta) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for linea in contenido.lines().skip(1) {
            if let Some(socket) = analizar_linea(linea, ipv6) {
                salida.push(socket);
            }
        }
    }
    salida
}

/// Analiza una línea de `/proc/net/tcp`.
///
/// Columnas: `sl local rem st tx:rx tr:when retrnsmt uid timeout inode …`
fn analizar_linea(linea: &str, ipv6: bool) -> Option<Socket> {
    let campos: Vec<&str> = linea.split_whitespace().collect();
    if campos.len() < 10 {
        return None;
    }
    let (direccion_hex, puerto_hex) = campos[1].split_once(':')?;
    let direccion = if ipv6 {
        IpAddr::V6(analizar_ipv6(direccion_hex)?)
    } else {
        IpAddr::V4(analizar_ipv4(direccion_hex)?)
    };
    Some(Socket {
        direccion,
        puerto: u16::from_str_radix(puerto_hex, 16).ok()?,
        estado: campos[3].to_string(),
        uid: campos[7].parse().unwrap_or(0),
        inodo: campos[9].parse().ok()?,
    })
}

/// El núcleo escribe la IPv4 como un `u32` en orden de máquina (little-endian).
fn analizar_ipv4(hexadecimal: &str) -> Option<Ipv4Addr> {
    let bruto = u32::from_str_radix(hexadecimal, 16).ok()?;
    Some(Ipv4Addr::from(bruto.swap_bytes()))
}

/// La IPv6 son cuatro palabras de 32 bits, cada una en orden de máquina.
fn analizar_ipv6(hexadecimal: &str) -> Option<Ipv6Addr> {
    if hexadecimal.len() != 32 {
        return None;
    }
    let mut octetos = [0u8; 16];
    for palabra in 0..4 {
        let trozo = &hexadecimal[palabra * 8..palabra * 8 + 8];
        let bruto = u32::from_str_radix(trozo, 16).ok()?.swap_bytes();
        octetos[palabra * 4..palabra * 4 + 4].copy_from_slice(&bruto.to_be_bytes());
    }
    Some(Ipv6Addr::from(octetos))
}

/// Busca en `/proc/*/fd` el proceso dueño de un inodo de socket.
///
/// Solo se ven los procesos del propio usuario, que es exactamente el caso que
/// importa: los conflictos de puerto los provoca casi siempre algo que ha
/// arrancado el propio usuario. Si el dueño es de otro usuario, se devuelve
/// `None` y la interfaz lo dice sin inventar.
fn proceso_de_inodo(inodo: u64) -> Option<(u32, Option<String>)> {
    let marca = format!("socket:[{inodo}]");
    let entradas = std::fs::read_dir("/proc").ok()?;

    for entrada in entradas.flatten() {
        let pid: u32 = match entrada.file_name().to_str().and_then(|n| n.parse().ok()) {
            Some(pid) => pid,
            None => continue,
        };
        let descriptores = match std::fs::read_dir(entrada.path().join("fd")) {
            Ok(d) => d,
            // Sin permiso: es de otro usuario.
            Err(_) => continue,
        };
        for descriptor in descriptores.flatten() {
            if let Ok(destino) = std::fs::read_link(descriptor.path()) {
                if destino.to_string_lossy() == marca {
                    return Some((pid, nombre_de_proceso(pid)));
                }
            }
        }
    }
    None
}

fn nombre_de_proceso(pid: u32) -> Option<String> {
    let ruta = Path::new("/proc").join(pid.to_string()).join("comm");
    std::fs::read_to_string(ruta)
        .ok()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn descodifica_ipv4_del_kernel() {
        // 0100007F es 127.0.0.1 escrito en orden de máquina.
        assert_eq!(analizar_ipv4("0100007F"), Some(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(analizar_ipv4("00000000"), Some(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn descodifica_ipv6_del_kernel() {
        let loopback = "00000000000000000000000001000000";
        assert_eq!(analizar_ipv6(loopback), Some(Ipv6Addr::LOCALHOST));
        assert_eq!(
            analizar_ipv6("00000000000000000000000000000000"),
            Some(Ipv6Addr::UNSPECIFIED)
        );
        assert_eq!(analizar_ipv6("corto"), None);
    }

    #[test]
    fn analiza_una_linea_real() {
        let linea = "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 45678 1 0000000000000000 100 0 0 10 0";
        let socket = analizar_linea(linea, false).unwrap();
        assert_eq!(socket.puerto, 8080);
        assert_eq!(socket.estado, ESTADO_LISTEN);
        assert_eq!(socket.uid, 1000);
        assert_eq!(socket.inodo, 45678);
    }

    #[test]
    fn detecta_un_puerto_que_esta_escuchando_de_verdad() {
        let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = escucha.local_addr().unwrap().port();

        assert!(esta_escuchando("127.0.0.1", puerto));
        assert!(matches!(
            comprobar_puerto_libre("127.0.0.1", puerto),
            Err(Error::PuertoOcupado { .. })
        ));

        drop(escucha);
        assert!(comprobar_puerto_libre("127.0.0.1", puerto).is_ok());
    }

    #[test]
    fn identifica_el_ocupante_como_este_proceso() {
        let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = escucha.local_addr().unwrap().port();
        let ocupante = quien_ocupa(puerto).expect("debería encontrar el socket");
        assert_eq!(ocupante.pid, Some(std::process::id()));
    }

    #[test]
    fn un_puerto_libre_no_figura_como_escuchando() {
        // Con las pruebas corriendo en paralelo, el puerto efímero que se acaba
        // de liberar puede reaparecer ocupado por otra. Se reintenta hasta dar
        // con uno que siga libre al mirarlo.
        let mut libre = None;
        for _ in 0..25 {
            let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
            let puerto = escucha.local_addr().unwrap().port();
            drop(escucha);
            if comprobar_puerto_libre("127.0.0.1", puerto).is_ok() {
                libre = Some(puerto);
                break;
            }
        }
        let puerto = libre.expect("no se encontró ningún puerto libre en 25 intentos");
        assert!(!esta_escuchando("127.0.0.1", puerto));
    }
}
