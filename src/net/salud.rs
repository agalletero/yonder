//! Sondas de salud: comprobar que el túnel **transporta**, no solo que existe.
//!
//! Que el puerto local escuche solo demuestra que `ssh` aceptó el reenvío. El
//! caso que de verdad duele es el túnel zombi: el puerto abre, el cliente
//! conecta, y al otro lado no hay nadie. Pasa cuando el destino del reenvío no
//! es donde el servicio remoto está realmente escuchando —por ejemplo `localhost`
//! cuando el contenedor está enganchado a un alias de IP concreto—. `ssh` no
//! tiene forma de saberlo y lo da por bueno.
//!
//! Estas sondas atraviesan el túnel de verdad, así que **tienen efecto al otro
//! lado**: son conexiones reales. Por eso corren con su propia cadencia, mucho
//! más lenta que el latido del supervisor, y por eso la comprobación barata
//! (¿escucha el puerto?) sigue siendo la que se ejecuta cada segundo.
//!
//! El cliente HTTP es mínimo a propósito. Traer una biblioteca entera —con su
//! TLS, su asincronía y sus doscientas dependencias— para mandar cuatro líneas
//! por un socket que ya está en `localhost` sería justo lo contrario de lo que
//! pide §2.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::modelo::Salud;

/// Tiempo máximo de una sonda. Va al bucle local: si tarda más, algo va mal.
pub const ESPERA: Duration = Duration::from_secs(4);

/// Resultado de sondear un túnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Veredicto {
    /// El túnel responde.
    Sano,
    /// El puerto local ni siquiera está en escucha.
    SinEscucha,
    /// El puerto escucha pero no responde: el zombi.
    Zombi(String),
}

impl Veredicto {
    pub fn sano(&self) -> bool {
        matches!(self, Veredicto::Sano)
    }

    /// Explicación para la interfaz.
    pub fn motivo(&self) -> Option<&str> {
        match self {
            Veredicto::Sano => None,
            Veredicto::SinEscucha => Some("el puerto local no está en escucha"),
            Veredicto::Zombi(detalle) => Some(detalle),
        }
    }
}

/// Sondea un túnel según su comprobación declarada.
///
/// `escuchando` es el resultado de la comprobación barata, que ya se ha hecho.
/// Si el puerto no escucha no hay nada más que sondear: conectar solo daría un
/// «conexión rechazada» que no añade información.
pub fn sondear(salud: &Salud, direccion: &str, puerto: u16, escuchando: bool) -> Veredicto {
    if !escuchando {
        return Veredicto::SinEscucha;
    }
    match salud {
        Salud::Escucha => Veredicto::Sano,
        Salud::Conecta => match conectar(direccion, puerto) {
            Ok(_) => Veredicto::Sano,
            Err(e) => Veredicto::Zombi(format!("el puerto escucha pero no se pudo conectar: {e}")),
        },
        Salud::Banner => sondear_banner(direccion, puerto),
        Salud::Http { ruta } => sondear_http(direccion, puerto, ruta),
    }
}

fn direccion_socket(direccion: &str, puerto: u16) -> std::io::Result<SocketAddr> {
    (direccion, puerto)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                format!("«{direccion}» no resuelve"),
            )
        })
}

fn conectar(direccion: &str, puerto: u16) -> std::io::Result<TcpStream> {
    let destino = direccion_socket(direccion, puerto)?;
    let flujo = TcpStream::connect_timeout(&destino, ESPERA)?;
    flujo.set_read_timeout(Some(ESPERA))?;
    flujo.set_write_timeout(Some(ESPERA))?;
    Ok(flujo)
}

/// Espera a que el servidor hable primero.
///
/// Sirve para protocolos que saludan al conectar: SSH manda su versión, Oracle
/// su cabecera. Si el túnel está zombi, la conexión se abre contra el `ssh`
/// local y se queda muda hasta que salta el tiempo máximo.
fn sondear_banner(direccion: &str, puerto: u16) -> Veredicto {
    let mut flujo = match conectar(direccion, puerto) {
        Ok(flujo) => flujo,
        Err(e) => {
            return Veredicto::Zombi(format!("el puerto escucha pero no se pudo conectar: {e}"))
        }
    };

    let mut byte = [0u8; 1];
    match flujo.read(&mut byte) {
        Ok(0) => Veredicto::Zombi(
            "el servidor cerró la conexión sin decir nada: el reenvío no llega al servicio"
                .to_string(),
        ),
        Ok(_) => Veredicto::Sano,
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            Veredicto::Zombi(format!(
                "el servidor no saludó en {} s: el túnel está en pie pero no transporta",
                ESPERA.as_secs()
            ))
        }
        Err(e) => Veredicto::Zombi(format!("error al leer del servicio: {e}")),
    }
}

/// Petición HTTP mínima por el túnel.
///
/// Se manda un `GET` con `Connection: close` y se lee la línea de estado. Vale
/// cualquier respuesta que no sea 4xx ni 5xx, igual que `curl -f`: un 404
/// significa que el servicio contesta pero la ruta está mal, y eso es un fallo
/// de configuración de la comprobación, no del túnel; conviene que se note.
fn sondear_http(direccion: &str, puerto: u16, ruta: &str) -> Veredicto {
    let inicio = Instant::now();
    let mut flujo = match conectar(direccion, puerto) {
        Ok(flujo) => flujo,
        Err(e) => {
            return Veredicto::Zombi(format!("el puerto escucha pero no se pudo conectar: {e}"))
        }
    };

    let peticion = format!(
        "GET {ruta} HTTP/1.1\r\n\
         Host: localhost:{puerto}\r\n\
         User-Agent: yonder/salud\r\n\
         Accept: */*\r\n\
         Connection: close\r\n\r\n"
    );
    if let Err(e) = flujo.write_all(peticion.as_bytes()) {
        return Veredicto::Zombi(format!("no se pudo enviar la petición: {e}"));
    }

    // Basta con la línea de estado; no hace falta leer el cuerpo entero.
    let mut respuesta = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        match flujo.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                respuesta.push(byte[0]);
                if respuesta.ends_with(b"\r\n") || respuesta.len() >= 128 {
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Veredicto::Zombi(format!(
                    "sin respuesta HTTP en {} s: el túnel está en pie pero no transporta",
                    ESPERA.as_secs()
                ));
            }
            Err(e) => return Veredicto::Zombi(format!("error al leer la respuesta: {e}")),
        }
    }

    if respuesta.is_empty() {
        return Veredicto::Zombi(
            "el servicio cerró sin responder: el reenvío no llega a ningún sitio".to_string(),
        );
    }

    let linea = String::from_utf8_lossy(&respuesta);
    match codigo_http(&linea) {
        Some(codigo) if (200..400).contains(&codigo) => {
            tracing::debug!(
                puerto,
                ruta,
                codigo,
                ms = inicio.elapsed().as_millis(),
                "sonda HTTP correcta"
            );
            Veredicto::Sano
        }
        Some(codigo) => Veredicto::Zombi(format!(
            "el servicio respondió {codigo} a «{ruta}»: contesta, pero no como se esperaba"
        )),
        None => Veredicto::Zombi(format!(
            "la respuesta no parece HTTP: «{}»",
            linea.trim().chars().take(60).collect::<String>()
        )),
    }
}

/// Extrae el código de una línea de estado `HTTP/1.1 200 OK`.
fn codigo_http(linea: &str) -> Option<u16> {
    let mut campos = linea.split_whitespace();
    let version = campos.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    campos.next()?.parse().ok()
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use std::net::TcpListener;

    /// Levanta un servidor de un solo uso y devuelve su puerto.
    fn servidor(
        respuesta: Option<&'static [u8]>,
        tras: Duration,
    ) -> (u16, std::thread::JoinHandle<()>) {
        let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = escucha.local_addr().unwrap().port();
        let hilo = std::thread::spawn(move || {
            if let Ok((mut flujo, _)) = escucha.accept() {
                std::thread::sleep(tras);
                if let Some(datos) = respuesta {
                    let _ = flujo.write_all(datos);
                }
                // Al salir del ámbito se cierra la conexión.
            }
        });
        (puerto, hilo)
    }

    #[test]
    fn sin_escucha_no_se_sondea_nada() {
        let veredicto = sondear(&Salud::Banner, "127.0.0.1", 1, false);
        assert_eq!(veredicto, Veredicto::SinEscucha);
    }

    #[test]
    fn la_salud_de_escucha_se_fia_de_la_comprobacion_barata() {
        assert_eq!(
            sondear(&Salud::Escucha, "127.0.0.1", 1, true),
            Veredicto::Sano
        );
    }

    #[test]
    fn banner_acepta_un_servidor_que_saluda() {
        let (puerto, hilo) = servidor(Some(b"SSH-2.0-OpenSSH_10.2\r\n"), Duration::ZERO);
        assert_eq!(
            sondear(&Salud::Banner, "127.0.0.1", puerto, true),
            Veredicto::Sano
        );
        hilo.join().unwrap();
    }

    #[test]
    fn banner_detecta_al_que_cierra_sin_decir_nada() {
        // Este es el zombi: acepta la conexión y no transporta nada.
        let (puerto, hilo) = servidor(None, Duration::ZERO);
        let veredicto = sondear(&Salud::Banner, "127.0.0.1", puerto, true);
        assert!(matches!(veredicto, Veredicto::Zombi(_)), "{veredicto:?}");
        assert!(veredicto.motivo().unwrap().contains("sin decir nada"));
        hilo.join().unwrap();
    }

    #[test]
    fn http_acepta_un_doscientos() {
        let (puerto, hilo) = servidor(
            Some(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"),
            Duration::ZERO,
        );
        assert_eq!(
            sondear(
                &Salud::Http {
                    ruta: "/api/health".into()
                },
                "127.0.0.1",
                puerto,
                true
            ),
            Veredicto::Sano
        );
        hilo.join().unwrap();
    }

    #[test]
    fn http_acepta_una_redireccion() {
        let (puerto, hilo) = servidor(
            Some(b"HTTP/1.1 302 Found\r\nLocation: /login\r\n\r\n"),
            Duration::ZERO,
        );
        assert_eq!(
            sondear(&Salud::Http { ruta: "/".into() }, "127.0.0.1", puerto, true),
            Veredicto::Sano
        );
        hilo.join().unwrap();
    }

    #[test]
    fn http_rechaza_un_error_del_servicio() {
        let (puerto, hilo) = servidor(
            Some(b"HTTP/1.1 503 Service Unavailable\r\n\r\n"),
            Duration::ZERO,
        );
        let veredicto = sondear(&Salud::Http { ruta: "/".into() }, "127.0.0.1", puerto, true);
        assert!(matches!(veredicto, Veredicto::Zombi(_)), "{veredicto:?}");
        assert!(veredicto.motivo().unwrap().contains("503"));
        hilo.join().unwrap();
    }

    #[test]
    fn http_detecta_al_que_acepta_y_no_contesta() {
        // El zombi de manual: el reenvío llega al ssh local y muere ahí.
        let (puerto, hilo) = servidor(None, Duration::ZERO);
        let veredicto = sondear(&Salud::Http { ruta: "/".into() }, "127.0.0.1", puerto, true);
        assert!(matches!(veredicto, Veredicto::Zombi(_)), "{veredicto:?}");
        hilo.join().unwrap();
    }

    #[test]
    fn http_avisa_si_la_respuesta_no_es_http() {
        let (puerto, hilo) = servidor(Some(b"esto no es HTTP\r\n"), Duration::ZERO);
        let veredicto = sondear(&Salud::Http { ruta: "/".into() }, "127.0.0.1", puerto, true);
        assert!(veredicto.motivo().unwrap().contains("no parece HTTP"));
        hilo.join().unwrap();
    }

    #[test]
    fn conecta_le_basta_con_que_alguien_acepte() {
        let (puerto, hilo) = servidor(None, Duration::ZERO);
        assert_eq!(
            sondear(&Salud::Conecta, "127.0.0.1", puerto, true),
            Veredicto::Sano
        );
        hilo.join().unwrap();
    }

    #[test]
    fn el_caso_real_del_guion_que_esto_sustituye() {
        // Reproduce el zombi documentado en tunel1.sh: el reenvío apunta a
        // «localhost» cuando el servicio remoto está enganchado a un alias de
        // IP concreto. `ssh` acepta el reenvío y el puerto local abre, pero
        // ninguna petición llega a ningún sitio.
        //
        // Con la comprobación barata este túnel se vería ACTIVO. Con la sonda
        // HTTP se ve como lo que es.
        let (puerto, hilo) = servidor(None, Duration::ZERO);

        let escuchando = true;
        assert_eq!(
            sondear(&Salud::Escucha, "127.0.0.1", puerto, escuchando),
            Veredicto::Sano,
            "la comprobación barata no puede distinguirlo, y por eso hace falta la otra"
        );

        let profunda = sondear(
            &Salud::Http {
                ruta: "/api/health".into(),
            },
            "127.0.0.1",
            puerto,
            escuchando,
        );
        assert!(
            matches!(profunda, Veredicto::Zombi(_)),
            "la sonda HTTP debería haberlo cazado: {profunda:?}"
        );
        hilo.join().unwrap();
    }

    #[test]
    fn extrae_el_codigo_de_la_linea_de_estado() {
        assert_eq!(codigo_http("HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(codigo_http("HTTP/1.0 404 Not Found"), Some(404));
        assert_eq!(codigo_http("SSH-2.0-OpenSSH"), None);
        assert_eq!(codigo_http(""), None);
    }
}
