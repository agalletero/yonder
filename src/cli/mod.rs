//! Interfaz de línea de órdenes (fase 1).
//!
//! Criterio de salida de la fase 1: el motor es usable a diario sin interfaz
//! gráfica. Todo lo que hace la GUI se puede hacer aquí.
//!
//! Salida sin colores y con los prefijos de §11. Las tablas van sin prefijo:
//! una etiqueta por fila las haría ilegibles.

use clap::{Parser, Subcommand};

use crate::config::{self, EstadoInclude};
use crate::error::{Error, Resultado};
use crate::modelo::Host;
use crate::registro::consola;
use crate::rutas;
use crate::ssh;
use crate::state::{machine::Estado, mensaje_completo, Motor};

#[derive(Debug, Parser)]
#[command(
    name = "yonder",
    version,
    about = "Gestor de túneles SSH: orquestador sobre el binario ssh del sistema",
    long_about = "Define, activa y supervisa túneles SSH.\n\n\
                  La definición vive en ~/.ssh/config.d/yonder.conf, así que todo\n\
                  túnel definido aquí funciona también con ssh, scp, rsync y\n\
                  VS Code Remote sin que esta aplicación esté corriendo.",
    disable_help_subcommand = true
)]
pub struct Argumentos {
    /// Registro detallado.
    #[arg(short = 'v', long, global = true)]
    pub verboso: bool,

    #[command(subcommand)]
    pub orden: Option<Orden>,
}

#[derive(Debug, Subcommand)]
pub enum Orden {
    /// Lista los túneles definidos y su estado.
    #[command(alias = "listar", alias = "ls")]
    List,

    /// Levanta un túnel o todos los de un alias.
    #[command(alias = "levantar")]
    Up {
        /// Alias del host o identificador de túnel.
        objetivo: String,
    },

    /// Baja un túnel o todos los de un alias.
    #[command(alias = "bajar")]
    Down {
        /// Alias del host o identificador de túnel.
        objetivo: String,
    },

    /// Resumen del estado, incluidos los maestros vivos.
    #[command(alias = "estado")]
    Status,

    /// Muestra los túneles que ya tenías definidos fuera del fichero propio.
    #[command(alias = "importar")]
    Import {
        /// Importa todos los encontrados sin preguntar.
        #[arg(long)]
        todos: bool,
    },

    /// Una pasada de reconciliación: levanta y repara lo que haga falta.
    ///
    /// Idempotente y pensada para ejecutarse periódicamente desde un
    /// temporizador. No deja ningún proceso corriendo.
    #[command(alias = "supervisar")]
    Supervise {
        /// No hace nada, solo dice qué haría.
        #[arg(long)]
        simular: bool,
    },

    /// Gestiona el temporizador de systemd que supervisa sin ventana abierta.
    #[command(alias = "servicio")]
    Service {
        #[command(subcommand)]
        que: OrdenServicio,
    },

    /// Retira los sockets de control huérfanos.
    #[command(alias = "limpiar")]
    Clean,

    /// Muestra las rutas XDG que usa la aplicación.
    #[command(alias = "rutas")]
    Paths,

    /// Verifica la clave de un host y la añade a known_hosts tras confirmarla.
    #[command(alias = "clave-host")]
    Hostkey {
        /// Host o alias a escanear.
        objetivo: String,
        /// Puerto SSH. Si el objetivo es un alias conocido, sale de su config.
        #[arg(short, long)]
        puerto: Option<u16>,
        /// Acepta sin preguntar. Úsalo solo si el fingerprint ya te consta.
        #[arg(long)]
        aceptar: bool,
    },

    /// Copia tu clave pública al host con ssh-copy-id.
    #[command(alias = "copiar-clave")]
    Copyid {
        /// Alias del host.
        objetivo: String,
        /// Clave pública a instalar. Por defecto, la primera disponible.
        #[arg(short, long)]
        clave: Option<String>,
    },

    /// Abre la interfaz gráfica.
    #[command(alias = "grafica")]
    Gui,
}

#[derive(Debug, Subcommand)]
pub enum OrdenServicio {
    /// Escribe las unidades de usuario y activa el temporizador.
    #[command(alias = "instalar")]
    Install {
        /// Cada cuánto reconcilia, en formato de systemd. Por defecto, 2min.
        #[arg(long)]
        intervalo: Option<String>,
    },
    /// Para el temporizador y borra las unidades.
    #[command(alias = "desinstalar")]
    Uninstall,
    /// Arranca el temporizador y hace una pasada ya.
    #[command(alias = "arrancar")]
    Start,
    /// Para el temporizador. Los túneles vivos siguen vivos.
    #[command(alias = "parar")]
    Stop,
    /// Dice si está instalado y activo.
    #[command(alias = "estado")]
    Status,
}

/// Ejecuta una orden de la CLI.
pub fn ejecutar(orden: Orden) -> Resultado<()> {
    match orden {
        Orden::List => listar(),
        Orden::Up { objetivo } => levantar(&objetivo),
        Orden::Down { objetivo } => bajar(&objetivo),
        Orden::Status => estado(),
        Orden::Import { todos } => importar(todos),
        Orden::Supervise { simular } => supervisar(simular),
        Orden::Service { que } => servicio(que),
        Orden::Clean => limpiar(),
        Orden::Paths => mostrar_rutas(),
        Orden::Hostkey {
            objetivo,
            puerto,
            aceptar,
        } => clave_host(&objetivo, puerto, aceptar),
        Orden::Copyid { objetivo, clave } => copiar_clave(&objetivo, clave.as_deref()),
        // La GUI la resuelve el binario: la biblioteca no depende de ella.
        Orden::Gui => Ok(()),
    }
}

fn motor() -> Resultado<Motor> {
    // En la terminal las contraseñas las pide `ssh` por el TTY, que es lo
    // correcto: el askpass gráfico solo tiene sentido con una ventana delante.
    Motor::nuevo(ssh::Entorno::terminal())
}

// --- Órdenes ---------------------------------------------------------------

fn listar() -> Resultado<()> {
    let mut motor = motor()?;
    motor.observar();

    let tuneles = motor.tuneles_visibles();
    if tuneles.is_empty() {
        consola::info("no hay ningún túnel definido todavía");
        consola::texto("");
        consola::texto(format!(
            "Defínelos en {} o ejecuta «yonder import» si ya tenías\n\
             reenvíos en ~/.ssh/config.",
            rutas::abreviar(&rutas::config_tuneles()?)
        ));
        return Ok(());
    }

    let mut tabla = Tabla::nueva(&["ESTADO", "TÚNEL", "REENVÍO", "DESTINO", "SALTOS"]);
    for tunel in &tuneles {
        let estado = motor.estado(&tunel.id());
        let host = motor.catalogo.host(&tunel.alias);
        tabla.fila(&[
            estado.estado.etiqueta().to_string(),
            tunel.alias.clone(),
            tunel.reenvio.descripcion(),
            host.map(|h| h.destino_completo()).unwrap_or_default(),
            host.map(|h| h.saltos.join(" → ")).unwrap_or_default(),
        ]);
    }
    consola::texto(tabla.render());
    consola::texto("");
    resumen(&motor);
    Ok(())
}

fn estado() -> Resultado<()> {
    let mut motor = motor()?;
    motor.observar();

    consola::texto(format!("OpenSSH:      {}", ssh::version_openssh()?));
    consola::texto(format!(
        "Configuración: {}",
        rutas::abreviar(&rutas::config_tuneles()?)
    ));
    consola::texto(format!(
        "Sockets:      {}",
        rutas::abreviar(&rutas::ejecucion()?)
    ));

    match config::estado_include()? {
        EstadoInclude::Presente => {
            consola::texto("Include:      presente en ~/.ssh/config");
        }
        EstadoInclude::Ausente | EstadoInclude::SinFichero => {
            consola::texto("Include:      AUSENTE de ~/.ssh/config");
            consola::aviso(
                "sin la línea Include, «ssh <alias>» desde la terminal no verá estos túneles; \
                 ejecuta «yonder import» para añadirla",
            );
        }
    }

    if !crate::net::puede_abrir_puertos_privilegiados() {
        consola::texto("Puertos <1024: no disponibles (falta CAP_NET_BIND_SERVICE)");
    } else {
        consola::texto("Puertos <1024: disponibles");
    }

    consola::texto("");

    let maestros = ssh::control::descubrir()?;
    if maestros.is_empty() {
        consola::texto("No hay ninguna conexión maestra abierta.");
    } else {
        let mut tabla = Tabla::nueva(&["MAESTRO", "PID", "SOCKET"]);
        for control in &maestros {
            let pid = control
                .comprobar()?
                .map(|p| p.to_string())
                .unwrap_or_else(|| "huérfano".to_string());
            tabla.fila(&[
                if control.alias.is_empty() {
                    "(desconocido)".to_string()
                } else {
                    control.alias.clone()
                },
                pid,
                rutas::abreviar(&control.socket),
            ]);
        }
        consola::texto(tabla.render());
    }

    consola::texto("");
    resumen(&motor);
    Ok(())
}

fn resumen(motor: &Motor) {
    let estados = motor.estados();
    let cuenta = |estado: Estado| estados.iter().filter(|e| e.estado == estado).count();
    consola::info(format!(
        "{} definidos · {} activos · {} degradados · {} reintentando · {} fallidos",
        estados.len(),
        cuenta(Estado::Activo),
        cuenta(Estado::Degradado),
        cuenta(Estado::Reintentando),
        cuenta(Estado::Fallido)
    ));

    for estado in estados.iter().filter(|e| e.estado.problematico()) {
        consola::aviso(format!(
            "{}: {} — {}",
            estado.id,
            estado.estado.etiqueta(),
            estado
                .ultimo_error
                .as_deref()
                .unwrap_or(estado.estado.explicacion())
        ));
    }
}

fn levantar(objetivo: &str) -> Resultado<()> {
    let mut motor = motor()?;
    let ids = resolver(&motor, objetivo)?;

    let mut fallos = 0;
    for id in &ids {
        match motor.levantar(id) {
            Ok(()) => {
                let estado = motor.estado(id);
                match estado.estado {
                    Estado::Activo => consola::info(format!("{id}: activo")),
                    Estado::Degradado => {
                        let motivo = estado
                            .ultimo_error
                            .as_deref()
                            .unwrap_or("el puerto local no escucha");
                        consola::aviso(format!("{id}: el reenvío está en pie pero {motivo}"));
                    }
                    otro => consola::info(format!("{id}: {}", otro.etiqueta())),
                }
            }
            Err(e) => {
                fallos += 1;
                consola::error(format!("{id}: {}", mensaje_completo(&e)));
            }
        }
    }

    if fallos > 0 {
        return Err(Error::OrdenFallida {
            orden: format!("yonder up {objetivo}"),
            codigo: 1,
            salida: format!("{fallos} de {} túneles no se pudieron levantar", ids.len()),
        });
    }
    Ok(())
}

fn bajar(objetivo: &str) -> Resultado<()> {
    let mut motor = motor()?;
    let ids = resolver(&motor, objetivo)?;
    for id in &ids {
        match motor.bajar(id) {
            Ok(()) => consola::info(format!("{id}: bajado")),
            Err(e) => consola::error(format!("{id}: {}", mensaje_completo(&e))),
        }
    }
    Ok(())
}

fn importar(todos: bool) -> Resultado<()> {
    let motor = motor()?;

    // Lo primero: la línea Include. Sin ella el criterio 7 de §12 no se cumple.
    match config::estado_include()? {
        EstadoInclude::Presente => {
            consola::info("~/.ssh/config ya incluye el fichero de túneles");
        }
        _ => {
            consola::info("añadiendo la línea Include a ~/.ssh/config");
            match config::asegurar_include()? {
                Some(respaldo) => consola::info(format!(
                    "copia de seguridad previa en {}",
                    rutas::abreviar(&respaldo)
                )),
                None => consola::info("~/.ssh/config creado"),
            }
        }
    }

    let candidatos = motor.importables();
    if candidatos.is_empty() {
        consola::info("no hay ningún host con reenvíos fuera del fichero propio");
        return Ok(());
    }

    consola::texto("");
    consola::texto("Hosts con reenvíos definidos fuera del fichero propio:");
    consola::texto("");
    let mut tabla = Tabla::nueva(&["ALIAS", "ORIGEN", "REENVÍOS"]);
    for host in &candidatos {
        tabla.fila(&[
            host.alias.clone(),
            origen_legible(host),
            host.reenvios
                .iter()
                .map(|r| r.descripcion())
                .collect::<Vec<_>>()
                .join(", "),
        ]);
    }
    consola::texto(tabla.render());
    consola::texto("");

    if !todos {
        consola::info("importar solo los muestra en la lista: NO se mueven de fichero ni se tocan");
        consola::info("ejecuta «yonder import --todos» para añadirlos todos");
        return Ok(());
    }

    for host in &candidatos {
        motor.importar(&host.alias)?;
        consola::info(format!("«{}» añadido a la lista", host.alias));
    }
    motor.base.marcar_importacion_hecha()?;
    Ok(())
}

fn origen_legible(host: &Host) -> String {
    match &host.origen {
        crate::modelo::Origen::Propio => "fichero propio".to_string(),
        crate::modelo::Origen::Ajeno(ruta) => rutas::abreviar(ruta),
    }
}

/// Una pasada de reconciliación (§3.3 sin ventana abierta).
///
/// Es la orden que dispara el temporizador. Mira qué debería estar arriba, lo
/// compara con lo que hay, y actúa. No deja nada corriendo: los maestros que
/// abre están desprendidos y le sobreviven, que es justo el modelo de §3.6.
fn supervisar(simular: bool) -> Resultado<()> {
    let mut motor = motor()?;
    motor.observar();

    let mut arreglados = 0;
    let mut fallidos = 0;
    let mut correctos = 0;

    for estado in motor.estados() {
        let id = estado.id.clone();
        match estado.estado {
            Estado::Activo => {
                correctos += 1;
                consola::texto(format!("  {id}  OK"));
            }
            // Maestro vivo pero el túnel no responde: rehacer el reenvío.
            Estado::Degradado => {
                let motivo = estado
                    .ultimo_error
                    .as_deref()
                    .unwrap_or("no responde")
                    .lines()
                    .next()
                    .unwrap_or("no responde")
                    .to_string();
                consola::texto(format!("  {id}  DEGRADADO → {motivo}"));
                if simular {
                    consola::texto("      (simulación: se repararía)");
                    continue;
                }
                match motor.reparar(&id).or_else(|_| motor.reparar_a_fondo(&id)) {
                    Ok(()) => {
                        arreglados += 1;
                        consola::texto("      → reparado");
                    }
                    Err(e) => {
                        fallidos += 1;
                        consola::error(format!("      → {}", mensaje_completo(&e)));
                    }
                }
            }
            // Se quería arriba y no hay maestro.
            Estado::Reintentando | Estado::Fallido => {
                consola::texto(format!("  {id}  CAÍDO → estableciendo"));
                if simular {
                    consola::texto("      (simulación: se levantaría)");
                    continue;
                }
                match motor.levantar(&id) {
                    Ok(()) => {
                        arreglados += 1;
                        consola::texto("      → arriba");
                    }
                    Err(e) => {
                        fallidos += 1;
                        consola::error(format!("      → {}", mensaje_completo(&e)));
                    }
                }
            }
            // `Definido` significa que el usuario no lo quiere arriba. Una
            // supervisión que levantara lo que alguien acaba de bajar sería
            // insufrible.
            Estado::Definido | Estado::Conectando | Estado::Cerrando => {}
        }
    }

    consola::info(format!(
        "{correctos} correctos · {arreglados} reparados · {fallidos} con fallo"
    ));
    if fallidos > 0 {
        return Err(Error::OrdenFallida {
            orden: "yonder supervise".to_string(),
            codigo: 1,
            salida: format!("{fallidos} túneles no se pudieron recuperar"),
        });
    }
    Ok(())
}

fn servicio(que: OrdenServicio) -> Resultado<()> {
    use crate::servicio as srv;

    match que {
        OrdenServicio::Install { intervalo } => {
            let unidades = srv::instalar(intervalo.as_deref())?;
            for unidad in &unidades {
                consola::info(format!("escrita {}", rutas::abreviar(unidad)));
            }
            consola::info("temporizador activo: los túneles se supervisan sin ventana abierta");
            consola::texto("");
            consola::texto(srv::aviso_linger());
        }
        OrdenServicio::Uninstall => {
            srv::desinstalar()?;
            consola::info("temporizador retirado; los túneles vivos siguen vivos");
        }
        OrdenServicio::Start => {
            srv::arrancar()?;
            consola::info("supervisión arrancada");
        }
        OrdenServicio::Stop => {
            srv::parar()?;
            consola::info("supervisión parada: no se reintentará nada");
            consola::info("los túneles vivos siguen vivos hasta que se caigan solos");
        }
        OrdenServicio::Status => {
            let estado = srv::estado()?;
            consola::texto(format!("Supervisión periódica: {}", estado.etiqueta()));
            if estado == srv::EstadoServicio::NoInstalado {
                consola::texto("");
                consola::texto("Instálala con:  yonder service install");
            }
        }
    }
    Ok(())
}

fn limpiar() -> Resultado<()> {
    let informe = ssh::control::limpiar_huerfanos()?;
    if informe.retirados.is_empty() {
        consola::info("no había ningún socket huérfano");
    } else {
        for socket in &informe.retirados {
            consola::info(format!("retirado {}", rutas::abreviar(socket)));
        }
    }
    if !informe.vivos.is_empty() {
        consola::info(format!("maestros vivos: {}", informe.vivos.join(", ")));
    }
    Ok(())
}

fn mostrar_rutas() -> Resultado<()> {
    let filas = [
        ("Configuración de túneles", rutas::config_tuneles()?),
        (
            "Configuración SSH del usuario",
            rutas::config_ssh_usuario()?,
        ),
        ("Preferencias", rutas::preferencias()?),
        ("Estado (SQLite)", rutas::base_de_datos()?),
        ("Sockets de control", rutas::ejecucion()?),
        ("Socket del askpass", rutas::socket_askpass()?),
        ("Registro", rutas::registro()?),
    ];
    let mut tabla = Tabla::nueva(&["USO", "RUTA"]);
    for (uso, ruta) in filas {
        tabla.fila(&[uso.to_string(), rutas::abreviar(&ruta)]);
    }
    consola::texto(tabla.render());
    Ok(())
}

fn clave_host(
    objetivo: &str,
    puerto_explicito: Option<u16>,
    aceptar_sin_preguntar: bool,
) -> Resultado<()> {
    let motor = motor()?;
    // Si es un alias conocido se usan su HostName y su Port reales; un puerto
    // dado a mano manda sobre todo lo demás.
    let (host, puerto) = match motor.catalogo.host(objetivo) {
        Some(definicion) => (
            definicion.destino().to_string(),
            puerto_explicito.or(definicion.puerto).unwrap_or(22),
        ),
        None => (objetivo.to_string(), puerto_explicito.unwrap_or(22)),
    };
    consola::info(format!("objetivo: {host} puerto {puerto}"));

    if ssh::hostkey::ya_conocido(&host, puerto)? {
        consola::info(format!("«{host}» ya está en known_hosts"));
        return Ok(());
    }

    consola::info(format!("escaneando las claves de «{host}»…"));
    let claves = ssh::hostkey::escanear(&host, puerto)?;

    consola::texto("");
    consola::texto(format!("Claves que presenta «{host}»:"));
    consola::texto("");
    let mut tabla = Tabla::nueva(&["ALGORITMO", "BITS", "FINGERPRINT"]);
    for clave in &claves {
        tabla.fila(&[
            clave.algoritmo(),
            clave.bits.to_string(),
            clave.fingerprint.clone(),
        ]);
    }
    consola::texto(tabla.render());
    consola::texto("");
    consola::aviso(
        "compara este fingerprint con el que te conste POR OTRO CANAL antes de aceptarlo",
    );

    if !aceptar_sin_preguntar && !preguntar("¿Aceptar estas claves?")? {
        consola::info("no se ha aceptado nada; known_hosts queda como estaba");
        return Err(Error::Cancelada);
    }

    ssh::hostkey::aceptar(&claves)?;
    consola::info(format!("{} claves añadidas a known_hosts", claves.len()));
    Ok(())
}

fn copiar_clave(objetivo: &str, clave: Option<&str>) -> Resultado<()> {
    let ruta = match clave {
        Some(ruta) => rutas::expandir(ruta),
        None => {
            let disponibles = ssh::copyid::claves_publicas_disponibles()?;
            let elegida = disponibles.first().cloned().ok_or_else(|| {
                Error::DefinicionInvalida(
                    "no hay ninguna clave pública en ~/.ssh; genera una con \
                     «ssh-keygen -t ed25519»"
                        .to_string(),
                )
            })?;
            consola::info(format!("usando {}", rutas::abreviar(&elegida)));
            elegida
        }
    };

    if ssh::copyid::es_clave_hardware(&ruta) {
        consola::aviso("es una clave respaldada por hardware: tendrás que tocar la llave física");
    }

    ssh::copyid::copiar_clave(objetivo, &ruta, &ssh::Entorno::terminal())?;
    consola::info(format!("clave instalada en «{objetivo}»"));

    if ssh::copyid::verificar_solo_clave(objetivo)? {
        consola::info("comprobado: ya se entra solo con clave pública");
        consola::info(format!(
            "si guardaste una contraseña de «{objetivo}» en el llavero, ya puedes borrarla"
        ));
    } else {
        consola::aviso(
            "la clave está instalada pero la entrada sin contraseña todavía no funciona",
        );
    }
    Ok(())
}

// --- Utilidades ------------------------------------------------------------

/// Traduce lo que escribe el usuario a identificadores de túnel.
///
/// Acepta un alias (levanta todos sus reenvíos) o un identificador exacto.
fn resolver(motor: &Motor, objetivo: &str) -> Resultado<Vec<String>> {
    let tuneles = motor.tuneles_visibles();

    if let Some(tunel) = tuneles.iter().find(|t| t.id() == objetivo) {
        return Ok(vec![tunel.id()]);
    }

    let del_alias: Vec<String> = tuneles
        .iter()
        .filter(|t| t.alias == objetivo)
        .map(|t| t.id())
        .collect();
    if !del_alias.is_empty() {
        return Ok(del_alias);
    }

    let parecidos: Vec<String> = tuneles
        .iter()
        .map(|t| t.alias.clone())
        .filter(|alias| alias.contains(objetivo) || objetivo.contains(alias.as_str()))
        .collect();

    if parecidos.is_empty() {
        Err(Error::TunelDesconocido(objetivo.to_string()))
    } else {
        Err(Error::DefinicionInvalida(format!(
            "«{objetivo}» no existe. ¿Querías decir: {}?",
            parecidos.join(", ")
        )))
    }
}

fn preguntar(pregunta: &str) -> Resultado<bool> {
    use std::io::Write as _;
    print!("{pregunta} [s/N] ");
    std::io::stdout().flush().map_err(Error::Io)?;
    let mut respuesta = String::new();
    std::io::stdin()
        .read_line(&mut respuesta)
        .map_err(Error::Io)?;
    Ok(matches!(
        respuesta.trim().to_ascii_lowercase().as_str(),
        "s" | "si" | "sí" | "y" | "yes"
    ))
}

/// Tabla de texto alineada, sin colores ni caracteres de dibujo.
struct Tabla {
    cabeceras: Vec<String>,
    filas: Vec<Vec<String>>,
}

impl Tabla {
    fn nueva(cabeceras: &[&str]) -> Tabla {
        Tabla {
            cabeceras: cabeceras.iter().map(|c| c.to_string()).collect(),
            filas: Vec::new(),
        }
    }

    fn fila(&mut self, celdas: &[String]) {
        self.filas.push(celdas.to_vec());
    }

    fn render(&self) -> String {
        let columnas = self.cabeceras.len();
        let mut anchos: Vec<usize> = self.cabeceras.iter().map(|c| c.chars().count()).collect();
        for fila in &self.filas {
            for (indice, celda) in fila.iter().enumerate().take(columnas) {
                anchos[indice] = anchos[indice].max(celda.chars().count());
            }
        }

        let componer = |celdas: &[String]| -> String {
            let mut linea = String::new();
            for (indice, celda) in celdas.iter().enumerate().take(columnas) {
                if indice + 1 == columnas {
                    linea.push_str(celda);
                } else {
                    let relleno = anchos[indice].saturating_sub(celda.chars().count());
                    linea.push_str(celda);
                    linea.push_str(&" ".repeat(relleno + 2));
                }
            }
            linea.trim_end().to_string()
        };

        let mut lineas = vec![componer(&self.cabeceras)];
        lineas.push(
            anchos
                .iter()
                .map(|ancho| "-".repeat(*ancho))
                .collect::<Vec<_>>()
                .join("  "),
        );
        for fila in &self.filas {
            lineas.push(componer(fila));
        }
        lineas.join("\n")
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn la_tabla_alinea_las_columnas() {
        let mut tabla = Tabla::nueva(&["A", "BB"]);
        tabla.fila(&["larguísimo".into(), "x".into()]);
        tabla.fila(&["c".into(), "y".into()]);
        let texto = tabla.render();
        let lineas: Vec<&str> = texto.lines().collect();
        assert_eq!(lineas.len(), 4);
        // La segunda columna arranca en la misma posición en todas las filas.
        let posicion =
            |linea: &str| linea.find(|c: char| !c.is_whitespace() && linea.starts_with(c));
        assert!(lineas[2].starts_with("larguísimo"));
        assert!(lineas[3].starts_with("c "));
        let _ = posicion;
    }

    #[test]
    fn la_tabla_no_deja_espacios_al_final() {
        let mut tabla = Tabla::nueva(&["A", "B"]);
        tabla.fila(&["uno".into(), "dos".into()]);
        for linea in tabla.render().lines() {
            assert_eq!(linea, linea.trim_end(), "hay relleno sobrante al final");
        }
    }

    #[test]
    fn la_ayuda_se_puede_construir() {
        // clap detecta en tiempo de ejecución los alias duplicados y los
        // argumentos mal declarados; esto los saca en las pruebas.
        use clap::CommandFactory;
        Argumentos::command().debug_assert();
    }
}
