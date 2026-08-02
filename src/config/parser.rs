//! Lectura de ficheros `ssh_config`.
//!
//! El parser es **conservador**: convierte el fichero en una lista de líneas
//! clasificadas y bloques, sin descartar nada. Las directivas que la aplicación
//! no entiende se conservan íntegras, igual que los comentarios y las líneas en
//! blanco. Esto es lo que permite cumplir la regla de §3.1 de preservar
//! comentarios y orden al reescribir el fichero propio.

use std::path::{Path, PathBuf};

use crate::error::{Error, Resultado};
use crate::modelo::{Host, Origen, Reenvio, Salud, TipoReenvio};

/// Clasificación de una línea del fichero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clase {
    Vacia,
    Comentario,
    /// Clave tal como aparece en el fichero y valor ya sin comillas externas.
    Directiva {
        clave: String,
        valor: String,
    },
}

/// Una línea del fichero, con su texto original intacto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linea {
    /// Texto exacto de la línea, sin el salto final.
    pub cruda: String,
    pub clase: Clase,
}

impl Linea {
    pub fn analizar(cruda: &str) -> Linea {
        let recortada = cruda.trim();
        let clase = if recortada.is_empty() {
            Clase::Vacia
        } else if recortada.starts_with('#') {
            Clase::Comentario
        } else {
            match dividir_directiva(recortada) {
                Some((clave, valor)) => Clase::Directiva { clave, valor },
                // Una línea que no es ni comentario ni directiva reconocible se
                // trata como comentario: se conserva, pero no se interpreta.
                None => Clase::Comentario,
            }
        };
        Linea {
            cruda: cruda.to_string(),
            clase,
        }
    }

    pub fn comentario(texto: &str) -> Linea {
        Linea::analizar(&format!("# {texto}"))
    }

    pub fn vacia() -> Linea {
        Linea {
            cruda: String::new(),
            clase: Clase::Vacia,
        }
    }

    /// Construye una directiva con la sangría estándar de cuatro espacios.
    pub fn directiva(clave: &str, valor: &str) -> Linea {
        Linea::analizar(&format!("    {clave} {valor}"))
    }

    /// Nombre de la directiva en minúsculas, para comparar sin distinguir
    /// mayúsculas como hace OpenSSH.
    pub fn clave_normalizada(&self) -> Option<String> {
        match &self.clase {
            Clase::Directiva { clave, .. } => Some(clave.to_ascii_lowercase()),
            _ => None,
        }
    }

    pub fn valor(&self) -> Option<&str> {
        match &self.clase {
            Clase::Directiva { valor, .. } => Some(valor),
            _ => None,
        }
    }

    pub fn es(&self, clave: &str) -> bool {
        self.clave_normalizada().as_deref() == Some(&clave.to_ascii_lowercase())
    }

    /// Texto de un comentario, sin la almohadilla ni los espacios.
    pub fn texto_comentario(&self) -> Option<&str> {
        if self.clase != Clase::Comentario {
            return None;
        }
        Some(self.cruda.trim().trim_start_matches('#').trim())
    }

    /// Especificación de salud si la línea es un marcador `# salud: …`.
    ///
    /// Va en un comentario a propósito: `ssh` lo ignora, así que la definición
    /// del túnel sigue estando en un solo sitio y `ssh <alias>` desde la
    /// terminal funciona exactamente igual (§3.1).
    pub fn marca_de_salud(&self) -> Option<&str> {
        self.texto_comentario()?
            .strip_prefix(Salud::MARCA)
            .map(str::trim)
    }
}

/// Separa `Clave valor`, `Clave=valor` o `Clave = valor`.
fn dividir_directiva(texto: &str) -> Option<(String, String)> {
    let bytes = texto.as_bytes();
    let fin_clave = bytes
        .iter()
        .position(|b| b.is_ascii_whitespace() || *b == b'=')
        .unwrap_or(bytes.len());
    if fin_clave == 0 {
        return None;
    }
    let clave = texto[..fin_clave].to_string();
    if !clave
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let resto = texto[fin_clave..].trim_start();
    let resto = resto.strip_prefix('=').unwrap_or(resto).trim();
    Some((clave, quitar_comillas(resto)))
}

fn quitar_comillas(texto: &str) -> String {
    let recortado = texto.trim();
    if recortado.len() >= 2 && recortado.starts_with('"') && recortado.ends_with('"') {
        recortado[1..recortado.len() - 1].to_string()
    } else {
        recortado.to_string()
    }
}

/// Separa el valor de `Host` en patrones, respetando comillas.
fn dividir_patrones(valor: &str) -> Vec<String> {
    let mut patrones = Vec::new();
    let mut actual = String::new();
    let mut entre_comillas = false;
    for c in valor.chars() {
        match c {
            '"' => entre_comillas = !entre_comillas,
            c if c.is_whitespace() && !entre_comillas => {
                if !actual.is_empty() {
                    patrones.push(std::mem::take(&mut actual));
                }
            }
            c => actual.push(c),
        }
    }
    if !actual.is_empty() {
        patrones.push(actual);
    }
    patrones
}

/// Encabezado de un bloque: `Host …` o `Match …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encabezado {
    Host {
        patrones: Vec<String>,
    },
    /// `Match` no se interpreta; solo se conserva para no romper el fichero.
    Match {
        condicion: String,
    },
}

/// Un bloque del fichero, con los comentarios que lo preceden.
#[derive(Debug, Clone)]
pub struct Bloque {
    /// Comentarios pegados justo encima del encabezado, sin línea en blanco de
    /// por medio. Se mueven y se borran con el bloque.
    pub comentarios: Vec<Linea>,
    pub encabezado: Encabezado,
    /// Línea del encabezado tal como está escrita.
    pub linea_encabezado: Linea,
    /// Todo lo que hay hasta el siguiente bloque, incluidos comentarios y
    /// líneas en blanco internas.
    pub cuerpo: Vec<Linea>,
}

impl Bloque {
    /// Alias gestionable: solo para `Host` con un único patrón sin comodines.
    ///
    /// Un `Host *` o un `Host a b c` se muestran pero no se gestionan: no
    /// identifican una máquina concreta a la que abrir un maestro.
    pub fn alias(&self) -> Option<&str> {
        match &self.encabezado {
            Encabezado::Host { patrones } if patrones.len() == 1 => {
                let patron = patrones[0].as_str();
                if patron.contains(['*', '?', '!']) {
                    None
                } else {
                    Some(patron)
                }
            }
            _ => None,
        }
    }

    /// Primera aparición de una directiva.
    pub fn buscar(&self, clave: &str) -> Option<&Linea> {
        self.cuerpo.iter().find(|l| l.es(clave))
    }

    /// Todos los valores de una directiva, en orden.
    pub fn valores(&self, clave: &str) -> Vec<&str> {
        self.cuerpo
            .iter()
            .filter(|l| l.es(clave))
            .filter_map(|l| l.valor())
            .collect()
    }

    fn valor_unico(&self, clave: &str) -> Option<String> {
        self.buscar(clave)
            .and_then(|l| l.valor())
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
    }

    /// Nota de la aplicación: comentario `# nota: …` dentro del bloque.
    pub fn nota(&self) -> Option<String> {
        self.cuerpo
            .iter()
            .chain(self.comentarios.iter())
            .filter(|l| l.clase == Clase::Comentario)
            .find_map(|l| {
                let texto = l.cruda.trim().trim_start_matches('#').trim();
                texto
                    .strip_prefix("nota:")
                    .map(|resto| resto.trim().to_string())
            })
            .filter(|n| !n.is_empty())
    }

    /// Reenvíos del bloque, en orden de fichero y con su salud asociada.
    ///
    /// Se recorre el cuerpo de una pasada en lugar de buscar cada directiva por
    /// separado: el marcador `# salud:` se refiere al reenvío que tiene debajo,
    /// así que hay que respetar el orden para saber a cuál pertenece.
    fn reenvios(&self, alias: &str) -> Vec<Reenvio> {
        let mut reenvios = Vec::new();
        let mut salud_pendiente: Option<Salud> = None;

        for linea in &self.cuerpo {
            if let Some(especificacion) = linea.marca_de_salud() {
                match Salud::analizar(especificacion) {
                    Ok(salud) => salud_pendiente = Some(salud),
                    Err(e) => tracing::warn!(
                        alias = %alias,
                        especificacion = %especificacion,
                        "marca de salud no válida, se usará la de por defecto: {e}"
                    ),
                }
                continue;
            }

            let tipo = match linea.clave_normalizada().as_deref() {
                Some("localforward") => TipoReenvio::Local,
                Some("remoteforward") => TipoReenvio::Remoto,
                Some("dynamicforward") => TipoReenvio::Dinamico,
                _ => continue,
            };
            let valor = match linea.valor() {
                Some(v) => v,
                None => continue,
            };

            match Reenvio::analizar(tipo, valor) {
                Ok(reenvio) => {
                    let salud = salud_pendiente.take().unwrap_or_default();
                    reenvios.push(reenvio.con_salud(salud));
                }
                Err(e) => {
                    // Una marca de salud huérfana no debe heredarla el reenvío
                    // siguiente, que no tiene nada que ver con ella.
                    salud_pendiente = None;
                    tracing::warn!(
                        alias = %alias,
                        valor = %valor,
                        "se ignora un reenvío no válido: {e}"
                    );
                }
            }
        }
        reenvios
    }

    /// Extrae el modelo tipado del bloque. `None` si no es un host gestionable.
    pub fn a_host(&self, origen: Origen) -> Option<Host> {
        let alias = self.alias()?.to_string();
        let reenvios = self.reenvios(&alias);

        let saltos = self
            .valor_unico("proxyjump")
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Some(Host {
            alias,
            hostname: self.valor_unico("hostname"),
            usuario: self.valor_unico("user"),
            puerto: self.valor_unico("port").and_then(|v| v.parse().ok()),
            saltos,
            identidades: self
                .valores("identityfile")
                .into_iter()
                .map(str::to_string)
                .collect(),
            reenvios,
            origen,
            nota: self.nota(),
        })
    }
}

/// Un fichero `ssh_config` analizado.
#[derive(Debug, Clone)]
pub struct Documento {
    pub ruta: PathBuf,
    /// Líneas anteriores al primer bloque (cabecera del fichero, `Include`…).
    pub preambulo: Vec<Linea>,
    pub bloques: Vec<Bloque>,
    /// `true` si el fichero terminaba con salto de línea. Se respeta al escribir.
    pub salto_final: bool,
}

impl Documento {
    pub fn vacio(ruta: impl Into<PathBuf>) -> Documento {
        Documento {
            ruta: ruta.into(),
            preambulo: Vec::new(),
            bloques: Vec::new(),
            salto_final: true,
        }
    }

    /// Lee y analiza un fichero. Si no existe, devuelve un documento vacío.
    pub fn leer(ruta: &Path) -> Resultado<Documento> {
        match std::fs::read_to_string(ruta) {
            Ok(contenido) => Ok(Documento::analizar(ruta, &contenido)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Documento::vacio(ruta)),
            Err(e) => Err(Error::leyendo(ruta, e)),
        }
    }

    pub fn analizar(ruta: &Path, contenido: &str) -> Documento {
        let salto_final = contenido.is_empty() || contenido.ends_with('\n');
        let mut documento = Documento::vacio(ruta);
        // Bandeja de líneas todavía sin asignar a ningún bloque.
        let mut pendientes: Vec<Linea> = Vec::new();

        for cruda in contenido.lines() {
            let linea = Linea::analizar(cruda);
            let encabezado = match &linea.clase {
                Clase::Directiva { clave, valor } => match clave.to_ascii_lowercase().as_str() {
                    "host" => Some(Encabezado::Host {
                        patrones: dividir_patrones(valor),
                    }),
                    "match" => Some(Encabezado::Match {
                        condicion: valor.clone(),
                    }),
                    _ => None,
                },
                _ => None,
            };

            match encabezado {
                Some(encabezado) => {
                    // Los comentarios pegados al encabezado (sin línea en
                    // blanco entre medias) pertenecen al bloque nuevo.
                    let corte = pendientes
                        .iter()
                        .rposition(|l| l.clase != Clase::Comentario)
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    let comentarios = pendientes.split_off(corte);
                    documento.volcar(&mut pendientes);
                    documento.bloques.push(Bloque {
                        comentarios,
                        encabezado,
                        linea_encabezado: linea,
                        cuerpo: Vec::new(),
                    });
                }
                None => pendientes.push(linea),
            }
        }
        documento.volcar(&mut pendientes);
        documento.salto_final = salto_final;
        documento
    }

    /// Envía las líneas pendientes al último bloque, o al preámbulo si aún no
    /// hay ninguno.
    fn volcar(&mut self, pendientes: &mut Vec<Linea>) {
        if pendientes.is_empty() {
            return;
        }
        match self.bloques.last_mut() {
            Some(bloque) => bloque.cuerpo.append(pendientes),
            None => self.preambulo.append(pendientes),
        }
    }

    /// Vuelve a componer el texto exacto del fichero.
    pub fn a_texto(&self) -> String {
        let mut lineas: Vec<&str> = Vec::new();
        for linea in &self.preambulo {
            lineas.push(&linea.cruda);
        }
        for bloque in &self.bloques {
            for comentario in &bloque.comentarios {
                lineas.push(&comentario.cruda);
            }
            lineas.push(&bloque.linea_encabezado.cruda);
            for linea in &bloque.cuerpo {
                lineas.push(&linea.cruda);
            }
        }
        let mut texto = lineas.join("\n");
        if self.salto_final && !texto.is_empty() {
            texto.push('\n');
        }
        texto
    }

    pub fn bloque(&self, alias: &str) -> Option<&Bloque> {
        self.bloques.iter().find(|b| b.alias() == Some(alias))
    }

    pub fn bloque_mut(&mut self, alias: &str) -> Option<&mut Bloque> {
        self.bloques.iter_mut().find(|b| b.alias() == Some(alias))
    }

    pub fn posicion(&self, alias: &str) -> Option<usize> {
        self.bloques.iter().position(|b| b.alias() == Some(alias))
    }

    /// Todos los hosts gestionables del documento.
    pub fn hosts(&self, origen: Origen) -> Vec<Host> {
        self.bloques
            .iter()
            .filter_map(|b| b.a_host(origen.clone()))
            .collect()
    }

    /// Rutas de los ficheros incluidos con `Include`, ya expandidas.
    ///
    /// OpenSSH resuelve las rutas relativas respecto a `~/.ssh`.
    pub fn includes(&self) -> Vec<PathBuf> {
        let base = crate::rutas::directorio_ssh().unwrap_or_default();
        self.preambulo
            .iter()
            .chain(self.bloques.iter().flat_map(|b| b.cuerpo.iter()))
            .filter(|l| l.es("include"))
            .filter_map(|l| l.valor())
            .flat_map(|valor| valor.split_whitespace().collect::<Vec<_>>())
            .map(|token| {
                let expandida = crate::rutas::expandir(token);
                if expandida.is_absolute() {
                    expandida
                } else {
                    base.join(expandida)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    const EJEMPLO: &str = "\
# Configuración de Alex
Include ~/.ssh/config.d/yonder.conf

# nota: entorno de preproducción
Host tunel-preprod
    HostName host.interno.example
    User usuario
    ProxyJump salto.example
    LocalForward 3000 localhost:3000
    # este comentario debe sobrevivir
    ExitOnForwardFailure yes

Host *
    ServerAliveInterval 15
";

    #[test]
    fn el_texto_se_reconstruye_byte_a_byte() {
        let doc = Documento::analizar(Path::new("/dev/null"), EJEMPLO);
        assert_eq!(doc.a_texto(), EJEMPLO);
    }

    #[test]
    fn separa_preambulo_y_bloques() {
        let doc = Documento::analizar(Path::new("/dev/null"), EJEMPLO);
        assert_eq!(doc.bloques.len(), 2);
        assert!(doc.preambulo.iter().any(|l| l.es("include")));
    }

    #[test]
    fn el_comentario_pegado_va_con_el_bloque() {
        let doc = Documento::analizar(Path::new("/dev/null"), EJEMPLO);
        let bloque = doc.bloque("tunel-preprod").unwrap();
        assert_eq!(bloque.comentarios.len(), 1);
        assert!(bloque.comentarios[0].cruda.contains("preproducción"));
        // La línea en blanco anterior se queda fuera del bloque.
        assert!(doc.bloques[0]
            .comentarios
            .iter()
            .all(|l| l.clase == Clase::Comentario));
    }

    #[test]
    fn extrae_el_modelo_tipado() {
        let doc = Documento::analizar(Path::new("/dev/null"), EJEMPLO);
        let host = doc
            .bloque("tunel-preprod")
            .unwrap()
            .a_host(Origen::Propio)
            .unwrap();
        assert_eq!(host.hostname.as_deref(), Some("host.interno.example"));
        assert_eq!(host.usuario.as_deref(), Some("usuario"));
        assert_eq!(host.saltos, vec!["salto.example"]);
        assert_eq!(host.reenvios.len(), 1);
        assert_eq!(host.nota.as_deref(), Some("entorno de preproducción"));
    }

    #[test]
    fn el_comodin_no_es_gestionable() {
        let doc = Documento::analizar(Path::new("/dev/null"), EJEMPLO);
        assert!(doc.bloques[1].alias().is_none());
        assert!(doc.bloques[1].a_host(Origen::Propio).is_none());
    }

    #[test]
    fn acepta_el_igual_como_separador() {
        let linea = Linea::analizar("  HostName=host.example");
        assert!(linea.es("hostname"));
        assert_eq!(linea.valor(), Some("host.example"));
    }

    #[test]
    fn quita_las_comillas_del_valor() {
        let linea = Linea::analizar(r#"IdentityFile "~/.ssh/mi clave""#);
        assert_eq!(linea.valor(), Some("~/.ssh/mi clave"));
    }

    #[test]
    fn varios_patrones_no_son_gestionables() {
        let doc = Documento::analizar(Path::new("/dev/null"), "Host uno dos\n    User usuario\n");
        assert!(doc.bloques[0].alias().is_none());
    }

    #[test]
    fn conserva_el_bloque_match() {
        let texto = "Host uno\n    User usuario\n\nMatch host *.interno\n    User root\n";
        let doc = Documento::analizar(Path::new("/dev/null"), texto);
        assert_eq!(doc.a_texto(), texto);
        assert!(matches!(
            doc.bloques[1].encabezado,
            Encabezado::Match { .. }
        ));
    }

    #[test]
    fn fichero_vacio() {
        let doc = Documento::analizar(Path::new("/dev/null"), "");
        assert_eq!(doc.a_texto(), "");
        assert!(doc.bloques.is_empty());
    }
}
