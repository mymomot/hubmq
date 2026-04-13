/// Source email IMAP — polling Gmail toutes les 30s, route sur NATS.
///
/// ## Flux de traitement
/// 1. Connexion IMAP TLS sur `imap.gmail.com:993`
/// 2. Search `UNSEEN FROM motreff@gmail.com` toutes les `poll_interval_secs` secondes
/// 3. Pour chaque message non-lu :
///    - Vérification allowlist (`From` doit être dans `allowed_from`)
///    - Parse subject → routing :
///      - `^HUBMQ_BOT_TOKEN\s+(\w+)` → publish `admin.bot.add` (NATS direct)
///      - `^\[(\w+)\]\s+(.+)` ou `^(\w+):\s+(.+)` → publish `user.inbox.<agent>`
///      - Sinon → log warn + marquer lu
///    - Marque le message comme lu (`\\Seen`) après traitement
///
/// ## Sécurité
/// - Emails hors allowlist → rejetés silencieusement (log info uniquement)
/// - Token Telegram extrait du body → JAMAIS loggé (audit hash uniquement)
/// - Subject vide → ignoré silencieusement
///
/// ## Robustesse
/// - Reconnexion automatique à chaque intervalle de polling (évite les sessions IMAP mortes)
/// - Erreur de connexion → log warn + retry au prochain intervalle
/// - Parse failure → marquer lu pour éviter re-traitement infini
use crate::{
    app_state::AppState,
    message::{Message, Severity},
};
use anyhow::Context as _;
use std::time::Duration;

/// Configuration pour la source email IMAP.
///
/// En pratique les credentials sont lus depuis le même credential SMTP (gmail-app-password).
#[derive(Debug, Clone)]
pub struct EmailSourceConfig {
    /// Hôte IMAP (ex : `"imap.gmail.com"`).
    pub imap_host: String,
    /// Port IMAP TLS (ex : 993).
    pub imap_port: u16,
    /// Identifiant IMAP (adresse email, ex : `"mymomot74@gmail.com"`).
    pub username: String,
    /// Mot de passe IMAP (app password Gmail).
    pub password: String,
    /// Liste d'expéditeurs autorisés (ex : `["motreff@gmail.com"]`).
    ///
    /// Les emails dont le `From` ne contient aucune de ces adresses sont ignorés.
    pub allowed_from: Vec<String>,
    /// Intervalle entre deux polls IMAP (secondes).
    pub poll_interval_secs: u64,
}

// ─── Parsing sujet ────────────────────────────────────────────────────────────

/// Résultat du parsing du sujet email.
#[derive(Debug, PartialEq)]
pub enum SubjectRoute {
    /// Admin : création d'un bot. Contient le nom du bot.
    Admin { bot_name: String },
    /// Message utilisateur routé vers un agent. Contient nom agent et corps message.
    UserMessage { agent: String, body: String },
    /// Sujet non reconnu — ignoré.
    Unknown,
}

/// Parse le sujet email pour déterminer le type de message.
///
/// # Ordre de priorité
/// 1. `HUBMQ_BOT_TOKEN <name>` → Admin
/// 2. `[agent] ...` → UserMessage
/// 3. `agent: ...` → UserMessage
/// 4. Tout le reste → Unknown
pub fn parse_subject(subject: &str) -> SubjectRoute {
    let subject = subject.trim();

    // 1. Admin : HUBMQ_BOT_TOKEN <name>
    if let Some(bot_name) = extract_admin_subject(subject) {
        return SubjectRoute::Admin { bot_name };
    }

    // 2. [agent] message
    if let Some((agent, body)) = extract_bracket_subject(subject) {
        return SubjectRoute::UserMessage { agent, body };
    }

    // 3. agent: message
    if let Some((agent, body)) = extract_colon_subject(subject) {
        return SubjectRoute::UserMessage { agent, body };
    }

    SubjectRoute::Unknown
}

fn extract_admin_subject(s: &str) -> Option<String> {
    // Format : HUBMQ_BOT_TOKEN <name>
    // Prédicat aligné sur admin::is_valid_name : [a-z0-9_-] uniquement (pas de majuscules)
    if let Some(rest) = s.strip_prefix("HUBMQ_BOT_TOKEN") {
        let name = rest.trim().to_string();
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Some(name);
        }
    }
    None
}

fn extract_bracket_subject(s: &str) -> Option<(String, String)> {
    // Format : [agent] message (ou [agent])
    if !s.starts_with('[') {
        return None;
    }
    let close = s.find(']')?;
    let agent = s[1..close].trim().to_string();
    if agent.is_empty() {
        return None;
    }
    let body = s[close + 1..].trim().to_string();
    if body.is_empty() {
        // Sujet vide après le tag → le body viendra du corps email
        return Some((agent, String::new()));
    }
    Some((agent, body))
}

fn extract_colon_subject(s: &str) -> Option<(String, String)> {
    // Format : agent: message
    let colon = s.find(':')?;
    let agent = s[..colon].trim().to_string();
    // L'agent doit être un identifiant simple (pas d'espaces)
    if agent.is_empty() || agent.contains(' ') {
        return None;
    }
    let body = s[colon + 1..].trim().to_string();
    if body.is_empty() {
        return None;
    }
    Some((agent, body))
}

/// Extrait le token Telegram depuis le corps d'un email admin.
///
/// Cherche un pattern `\d{8,}:[A-Za-z0-9_-]{30,}`.
/// Retourne `None` si aucun token valide n'est trouvé.
///
/// Le token n'est JAMAIS loggé (responsabilité de l'appelant).
pub fn extract_token_from_body(body: &str) -> Option<String> {
    // Recherche manuelle pour éviter la dépendance `regex` (non présente dans le workspace).
    // Pattern : <digits 8+>:<alphanum+dash+underscore 30+>
    for line in body.lines() {
        for word in line.split_whitespace() {
            if let Some(colon_pos) = word.find(':') {
                let digits_part = &word[..colon_pos];
                let rest = &word[colon_pos + 1..];
                if digits_part.len() >= 8
                    && digits_part.chars().all(|c| c.is_ascii_digit())
                    && rest.len() >= 30
                    && rest.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                {
                    return Some(word.to_string());
                }
            }
        }
    }
    None
}

// ─── Vérification allowlist ───────────────────────────────────────────────────

/// Extrait l'adresse email brute depuis un header `From`.
///
/// Gère les formats :
/// - `"Display Name <user@domain>"` → `user@domain`
/// - `"<user@domain>"` → `user@domain`
/// - `"user@domain"` → `user@domain` (retourné tel quel, trimé)
///
/// Utilise `rfind` pour le `<` afin de gérer les guillemets dans le display name.
fn extract_email_addr(from: &str) -> &str {
    if let (Some(open), Some(close)) = (from.rfind('<'), from.rfind('>')) {
        if open < close {
            return from[open + 1..close].trim();
        }
    }
    from.trim()
}

/// Vérifie si l'adresse `From` est dans la liste autorisée.
///
/// Extrait l'adresse brute (angle-bracket si présents) puis compare avec `==` exact
/// insensible à la casse — protège contre les attaques par substring spoofing du type
/// `"attacker@motreff@gmail.com.evil.com"`.
pub fn is_from_allowed(from_header: &str, allowed: &[String]) -> bool {
    let addr = extract_email_addr(from_header);
    let addr_lower = addr.to_lowercase();
    allowed.iter().any(|a| addr_lower == a.to_lowercase())
}

// ─── Boucle principale ────────────────────────────────────────────────────────

/// Lance la boucle de polling IMAP.
///
/// Bloque indéfiniment — à lancer dans une tâche Tokio dédiée.
/// Les erreurs de connexion IMAP sont loguées et entraînent un retry
/// au prochain intervalle (pas de panique).
///
/// # Effets de bord
/// - Publie sur NATS : `admin.bot.add` ou `user.inbox.<agent>` selon le routing
/// - Marque les emails traités comme lus (`\\Seen`)
/// - Enregistre des entrées d'audit pour chaque action significative
pub async fn run(state: AppState, cfg: EmailSourceConfig) {
    let interval = Duration::from_secs(cfg.poll_interval_secs);
    tracing::info!(
        username = %cfg.username,
        host = %cfg.imap_host,
        port = cfg.imap_port,
        interval_secs = cfg.poll_interval_secs,
        "source email IMAP démarrée"
    );

    loop {
        // Chaque poll est une nouvelle connexion IMAP (évite les sessions périmées)
        let state_c = state.clone();
        let cfg_c = cfg.clone();

        let result = tokio::task::spawn_blocking(move || {
            poll_once(&state_c, &cfg_c)
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(err = ?e, "poll IMAP échoué — retry dans {}s", cfg.poll_interval_secs);
            }
            Err(e) => {
                tracing::warn!(err = ?e, "spawn_blocking IMAP paniqué — retry");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// Un cycle de polling IMAP (synchrone, à appeler dans spawn_blocking).
///
/// Ouvre une session IMAP, cherche les emails non-lus de l'allowlist,
/// les traite, et ferme la session.
///
/// # Erreurs
/// Retourne une erreur si la connexion IMAP échoue ou si NATS est injoignable.
fn poll_once(state: &AppState, cfg: &EmailSourceConfig) -> anyhow::Result<()> {
    // Connexion IMAP TLS
    let tls = native_tls::TlsConnector::builder()
        .build()
        .context("construction TLS connector IMAP")?;

    let client = imap::connect(
        (cfg.imap_host.as_str(), cfg.imap_port),
        &cfg.imap_host,
        &tls,
    )
    .context("connexion IMAP TLS")?;

    let mut session = client
        .login(&cfg.username, &cfg.password)
        .map_err(|(e, _)| anyhow::anyhow!("login IMAP échoué: {}", e))?;

    session
        .select("INBOX")
        .context("sélection INBOX IMAP")?;

    // Recherche des emails non-lus
    // On ne filtre pas par From ici (le crate imap peut avoir des limites avec SEARCH FROM)
    // On filtre côté Rust après fetch — plus robuste
    let uids = session
        .uid_search("UNSEEN")
        .context("IMAP SEARCH UNSEEN")?;

    if uids.is_empty() {
        session.logout().ok();
        return Ok(());
    }

    tracing::debug!(count = uids.len(), "emails non-lus trouvés");

    for uid in &uids {
        if let Err(e) = process_email(&mut session, state, cfg, *uid) {
            tracing::warn!(uid = uid, err = ?e, "traitement email uid={} échoué", uid);
            // Marquer lu même en cas d'erreur pour éviter re-traitement infini
            let uid_str = uid.to_string();
            session
                .uid_store(&uid_str, "+FLAGS (\\Seen)")
                .ok();
        }
    }

    session.logout().ok();
    Ok(())
}

/// Traite un seul email identifié par son UID.
///
/// Fetch, parse, route vers NATS, marque comme lu.
fn process_email(
    session: &mut imap::Session<native_tls::TlsStream<std::net::TcpStream>>,
    state: &AppState,
    cfg: &EmailSourceConfig,
    uid: u32,
) -> anyhow::Result<()> {
    let uid_str = uid.to_string();

    // Fetch de l'envelope + body[text]
    let messages = session
        .uid_fetch(&uid_str, "(RFC822 ENVELOPE)")
        .context("fetch email RFC822")?;

    let msg = messages
        .iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("fetch vide pour uid={}", uid))?;

    // Extraction des headers depuis RFC822
    let raw_body = msg
        .body()
        .ok_or_else(|| anyhow::anyhow!("pas de body RFC822 pour uid={}", uid))?;

    let (headers, body_text) = parse_rfc822(raw_body)?;

    // Vérification allowlist
    let from_header = headers.from.unwrap_or_default();
    if !is_from_allowed(&from_header, &cfg.allowed_from) {
        tracing::info!(
            from = %from_header,
            uid = uid,
            "email rejeté — From non autorisé"
        );
        // Marquer comme lu pour éviter re-poll
        session
            .uid_store(&uid_str, "+FLAGS (\\Seen)")
            .context("mark seen — from non autorisé")?;
        return Ok(());
    }

    let subject = headers.subject.unwrap_or_default();
    let message_id = headers.message_id.unwrap_or_default();

    tracing::info!(
        from = %from_header,
        subject = %subject,
        uid = uid,
        "email autorisé reçu"
    );

    // Routing selon le sujet
    match parse_subject(&subject) {
        SubjectRoute::Admin { bot_name } => {
            handle_admin_email(state, &from_header, &message_id, &subject, &bot_name, &body_text)?;
        }
        SubjectRoute::UserMessage { agent, body: subject_body } => {
            // Si le body du sujet est vide, utiliser le corps de l'email
            let content = if subject_body.is_empty() {
                body_text.trim().to_string()
            } else {
                subject_body
            };
            handle_user_email(state, &from_header, &message_id, &subject, &agent, &content)?;
        }
        SubjectRoute::Unknown => {
            tracing::warn!(
                subject = %subject,
                from = %from_header,
                uid = uid,
                "email ignoré — sujet non reconnu"
            );
            // Marquer lu pour ne pas re-traiter
            session
                .uid_store(&uid_str, "+FLAGS (\\Seen)")
                .context("mark seen — sujet inconnu")?;
            return Ok(());
        }
    }

    // Marquer comme lu après traitement réussi
    session
        .uid_store(&uid_str, "+FLAGS (\\Seen)")
        .context("mark seen — traitement OK")?;

    Ok(())
}

/// Traite un email admin (HUBMQ_BOT_TOKEN <bot_name>).
///
/// Extrait le token depuis le body, construit le payload `admin.bot.add`,
/// publie sur NATS de manière synchrone via `Handle::current().block_on`.
///
/// Le token n'est JAMAIS loggé.
fn handle_admin_email(
    state: &AppState,
    from: &str,
    message_id: &str,
    subject: &str,
    bot_name: &str,
    body: &str,
) -> anyhow::Result<()> {
    let token = extract_token_from_body(body)
        .ok_or_else(|| anyhow::anyhow!("token Telegram introuvable dans le body de l'email admin"))?;

    // Construction payload admin.bot.add (étendu avec requester_email)
    let payload = serde_json::json!({
        "name": bot_name,
        "target_agent": bot_name,
        "token": token,
        "allowed_chat_ids": [],
        "requester_email": from,
        "email_msg_id": message_id,
        "email_subject": subject,
    });

    let payload_bytes = serde_json::to_vec(&payload)
        .context("sérialisation payload admin.bot.add")?;

    // Publier sur NATS depuis un contexte synchrone via block_on sur le runtime Tokio courant
    // Utiliser tokio::runtime::Handle pour accéder au runtime depuis spawn_blocking
    let nats_js = state.nats.js.clone();
    tokio::runtime::Handle::current()
        .block_on(async move {
            nats_js
                .publish("admin.bot.add", payload_bytes.into())
                .await
                .map_err(|e| anyhow::anyhow!("publish admin.bot.add: {}", e))
        })?;

    tracing::info!(
        bot_name = %bot_name,
        from = %from,
        "email admin traité — publish admin.bot.add (token non loggé)"
    );

    Ok(())
}

/// Traite un email utilisateur routé vers un agent.
///
/// Publie sur `user.inbox.<agent>` avec les métadonnées email pour le routing retour.
fn handle_user_email(
    state: &AppState,
    from: &str,
    message_id: &str,
    subject: &str,
    agent: &str,
    content: &str,
) -> anyhow::Result<()> {
    if content.is_empty() {
        tracing::warn!(agent = %agent, from = %from, "email user ignoré — body vide");
        return Ok(());
    }

    let mut m = Message::new(
        "email",
        Severity::P2,
        format!("email from {}", from),
        content.to_string(),
    );
    m.tags = vec!["email".into(), "upstream".into()];
    m.meta.insert("source".into(), "email".into());
    m.meta.insert("email_from".into(), from.to_string());
    m.meta.insert("email_msg_id".into(), message_id.to_string());
    m.meta.insert("email_subject".into(), subject.to_string());
    m.meta.insert("target_agent".into(), agent.to_string());

    let nats_subject = format!("user.inbox.{}", agent);
    let payload = serde_json::to_vec(&m)
        .context("sérialisation message user email")?;

    let nats_js = state.nats.js.clone();
    tokio::runtime::Handle::current()
        .block_on(async move {
            nats_js
                .publish(nats_subject, payload.into())
                .await
                .map_err(|e| anyhow::anyhow!("publish user.inbox.<agent>: {}", e))
        })?;

    tracing::info!(
        agent = %agent,
        from = %from,
        msg_id = %m.id,
        "email user publié sur user.inbox.{}", agent
    );

    Ok(())
}

// ─── Parsing RFC822 ───────────────────────────────────────────────────────────

/// Headers extraits d'un message RFC822.
#[derive(Debug, Default)]
struct EmailHeaders {
    from: Option<String>,
    subject: Option<String>,
    message_id: Option<String>,
    in_reply_to: Option<String>,
}

/// Parse les headers et le body d'un message RFC822 brut.
///
/// Retourne les headers extraits et le body en texte brut.
/// Pour les messages multipart, extrait uniquement `text/plain`.
/// Pour les messages non-multipart, retourne le body entier.
fn parse_rfc822(raw: &[u8]) -> anyhow::Result<(EmailHeaders, String)> {
    let raw_str = std::str::from_utf8(raw)
        .unwrap_or("")
        .to_string();

    let mut headers = EmailHeaders::default();
    let mut body_start = 0;
    let mut in_headers = true;
    let mut current_header: Option<(String, String)> = None;
    let mut header_lines: Vec<(String, String)> = Vec::new();

    let lines: Vec<&str> = raw_str.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if in_headers {
            if line.is_empty() {
                // Fin des headers
                if let Some(h) = current_header.take() {
                    header_lines.push(h);
                }
                body_start = i + 1;
                in_headers = false;
                continue;
            }

            // Continuation de header (ligne commence par espace ou tab)
            if line.starts_with(' ') || line.starts_with('\t') {
                if let Some((_, ref mut val)) = current_header {
                    val.push(' ');
                    val.push_str(line.trim());
                }
                continue;
            }

            // Nouveau header
            if let Some(h) = current_header.take() {
                header_lines.push(h);
            }

            if let Some(colon_pos) = line.find(':') {
                let name = line[..colon_pos].trim().to_lowercase();
                let val = line[colon_pos + 1..].trim().to_string();
                current_header = Some((name, val));
            }
        }
    }

    // Dernier header si on n'a pas rencontré la ligne vide
    if let Some(h) = current_header.take() {
        header_lines.push(h);
    }

    // Extraction des headers pertinents
    for (name, val) in &header_lines {
        match name.as_str() {
            "from" => headers.from = Some(val.clone()),
            "subject" => headers.subject = Some(decode_mime_header(val)),
            "message-id" => headers.message_id = Some(val.clone()),
            "in-reply-to" => headers.in_reply_to = Some(val.clone()),
            _ => {}
        }
    }

    // Extraction du body
    let body_lines = &lines[body_start..];
    let body_raw = body_lines.join("\n");

    // Détection multipart
    let content_type = header_lines
        .iter()
        .find(|(n, _)| n == "content-type")
        .map(|(_, v)| v.to_lowercase())
        .unwrap_or_default();

    let body_text = if content_type.contains("multipart/") {
        // Extraction de la partie text/plain
        extract_plain_text_from_multipart(&body_raw, &content_type)
            .unwrap_or_default()
    } else if content_type.contains("text/plain") || content_type.is_empty() {
        // Body direct
        decode_transfer_encoding(&body_raw, &header_lines)
    } else {
        // Type non supporté (HTML seul, etc.) → body brut
        body_raw
    };

    Ok((headers, body_text))
}

/// Extraction simplifiée de la partie text/plain d'un message multipart.
///
/// Cherche la boundary dans Content-Type, découpe le body, retourne
/// la première partie `Content-Type: text/plain`.
fn extract_plain_text_from_multipart(body: &str, content_type: &str) -> Option<String> {
    // Extraction de la boundary
    let boundary = content_type
        .split(';')
        .find_map(|part| {
            let p = part.trim();
            p.strip_prefix("boundary=").map(|b| b.trim_matches('"').to_string())
        })?;

    let delimiter = format!("--{}", boundary);
    let parts: Vec<&str> = body.split(&delimiter).collect();

    for part in &parts[1..] {
        // Chercher Content-Type: text/plain dans les headers de cette partie
        let part_lower = part.to_lowercase();
        if part_lower.contains("content-type: text/plain") || part_lower.contains("content-type:text/plain") {
            // Extraire le body de cette partie (après la ligne vide)
            if let Some(blank_pos) = part.find("\n\n") {
                return Some(part[blank_pos + 2..].trim().to_string());
            } else if let Some(blank_pos) = part.find("\r\n\r\n") {
                return Some(part[blank_pos + 4..].trim().to_string());
            }
        }
    }

    None
}

/// Décode le contenu selon le Content-Transfer-Encoding.
///
/// Supporte `quoted-printable` (Gmail par défaut) et `base64`.
/// Les autres encodages retournent le body brut.
fn decode_transfer_encoding(body: &str, headers: &[(String, String)]) -> String {
    let encoding = headers
        .iter()
        .find(|(n, _)| n == "content-transfer-encoding")
        .map(|(_, v)| v.to_lowercase())
        .unwrap_or_default();

    match encoding.trim() {
        "base64" => {
            let joined: String = body.lines().collect::<String>();
            // Décodage base64 basique — pas de dépendance externe
            // On retourne le body brut si le décodage échoue
            match base64_decode(&joined) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => body.to_string(),
            }
        }
        "quoted-printable" => decode_quoted_printable(body),
        _ => body.to_string(),
    }
}

/// Décode le format quoted-printable.
fn decode_quoted_printable(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '=' {
            // Ligne soft-wrap : `=\n` → ignorer
            match chars.peek() {
                Some('\n') | Some('\r') => {
                    chars.next();
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                }
                _ => {
                    // Séquence hex : `=XX`
                    let h1 = chars.next().unwrap_or(' ');
                    let h2 = chars.next().unwrap_or(' ');
                    if let Ok(byte) = u8::from_str_radix(&format!("{}{}", h1, h2), 16) {
                        result.push(byte as char);
                    } else {
                        result.push('=');
                        result.push(h1);
                        result.push(h2);
                    }
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Décodage base64 minimal sans dépendance externe.
fn base64_decode(input: &str) -> anyhow::Result<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut lookup = [255u8; 256];
    for (i, &b) in TABLE.iter().enumerate() {
        lookup[b as usize] = i as u8;
    }

    let input: Vec<u8> = input.bytes().filter(|&b| b != b'\n' && b != b'\r' && b != b' ').collect();
    let mut output = Vec::new();

    let mut i = 0;
    while i + 3 < input.len() {
        let b0 = lookup[input[i] as usize];
        let b1 = lookup[input[i + 1] as usize];
        let b2 = lookup[input[i + 2] as usize];
        let b3 = lookup[input[i + 3] as usize];

        if b0 == 255 || b1 == 255 {
            anyhow::bail!("base64 invalide");
        }

        output.push((b0 << 2) | (b1 >> 4));
        if b2 != 255 && input[i + 2] != b'=' {
            output.push((b1 << 4) | (b2 >> 2));
        }
        if b3 != 255 && input[i + 3] != b'=' {
            output.push((b2 << 6) | b3);
        }

        i += 4;
    }

    Ok(output)
}

/// Décode un header MIME encodé (ex : `=?UTF-8?B?...?=` ou `=?UTF-8?Q?...?=`).
///
/// Retourne le texte brut si le header n'est pas encodé en MIME.
fn decode_mime_header(header: &str) -> String {
    // Format : =?charset?encoding?encoded_text?=
    let mut result = String::new();
    let mut remaining = header;

    while let Some(start) = remaining.find("=?") {
        // Texte avant le mot MIME
        result.push_str(&remaining[..start]);
        remaining = &remaining[start + 2..];

        // Chercher charset, encoding, text
        if let Some(q1) = remaining.find('?') {
            let _charset = &remaining[..q1];
            remaining = &remaining[q1 + 1..];

            if let Some(q2) = remaining.find('?') {
                let encoding = &remaining[..q2];
                remaining = &remaining[q2 + 1..];

                if let Some(q3) = remaining.find("?=") {
                    let encoded = &remaining[..q3];
                    remaining = remaining[q3 + 2..].trim_start_matches(' ');

                    let decoded = match encoding.to_uppercase().as_str() {
                        "B" => base64_decode(encoded)
                            .ok()
                            .and_then(|b| String::from_utf8(b).ok())
                            .unwrap_or_else(|| encoded.to_string()),
                        "Q" => decode_quoted_printable(&encoded.replace('_', " ")),
                        _ => encoded.to_string(),
                    };
                    result.push_str(&decoded);
                    continue;
                }
            }
        }

        // Parsing échoué — réinsérer le marqueur
        result.push_str("=?");
    }

    result.push_str(remaining);
    result
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_subject ──────────────────────────────────────────────────────────

    #[test]
    fn parse_subject_admin_token() {
        let route = parse_subject("HUBMQ_BOT_TOKEN llmcore");
        assert_eq!(route, SubjectRoute::Admin { bot_name: "llmcore".into() });
    }

    #[test]
    fn parse_subject_admin_token_with_dash() {
        let route = parse_subject("HUBMQ_BOT_TOKEN my-bot");
        assert_eq!(route, SubjectRoute::Admin { bot_name: "my-bot".into() });
    }

    #[test]
    fn parse_subject_admin_token_with_spaces() {
        // Extra spaces autour du nom — le trim produit un nom valide "llmcore"
        let route = parse_subject("HUBMQ_BOT_TOKEN  llmcore  ");
        assert_eq!(route, SubjectRoute::Admin { bot_name: "llmcore".into() });
    }

    #[test]
    fn parse_subject_admin_token_name_with_inner_spaces_rejected() {
        // Un nom avec espaces internes n'est pas valide
        let route = parse_subject("HUBMQ_BOT_TOKEN bot name");
        // "bot name" : le .chars().all détecte l'espace → Unknown
        assert_eq!(route, SubjectRoute::Unknown);
    }

    #[test]
    fn parse_subject_bracket_agent_message() {
        let route = parse_subject("[llmcore] explique CAP theorem");
        assert_eq!(
            route,
            SubjectRoute::UserMessage {
                agent: "llmcore".into(),
                body: "explique CAP theorem".into(),
            }
        );
    }

    #[test]
    fn parse_subject_bracket_empty_body() {
        // [agent] sans body — le body sera lu depuis le corps de l'email
        let route = parse_subject("[llmcore]");
        assert_eq!(
            route,
            SubjectRoute::UserMessage {
                agent: "llmcore".into(),
                body: "".into(),
            }
        );
    }

    #[test]
    fn parse_subject_colon_format() {
        let route = parse_subject("llmcore: quelle est la différence entre TCP et UDP");
        assert_eq!(
            route,
            SubjectRoute::UserMessage {
                agent: "llmcore".into(),
                body: "quelle est la différence entre TCP et UDP".into(),
            }
        );
    }

    #[test]
    fn parse_subject_unknown() {
        let route = parse_subject("Bonjour, voici ma question");
        assert_eq!(route, SubjectRoute::Unknown);
    }

    #[test]
    fn parse_subject_empty() {
        let route = parse_subject("");
        assert_eq!(route, SubjectRoute::Unknown);
    }

    // ── extract_token_from_body ────────────────────────────────────────────────

    #[test]
    fn extract_token_from_body_valid() {
        let body = "Voici le token :\n1234567890:AABBCCDDEE_FF-GGHHIIJJKKLLMMNNOOPPQQRRSSTTUUVVWWXXYYZZaa\n\nBonne chance !";
        let token = extract_token_from_body(body);
        assert!(token.is_some());
        let t = token.unwrap();
        assert!(t.starts_with("1234567890:"));
        assert!(t.len() >= 8 + 1 + 30);
    }

    #[test]
    fn extract_token_from_body_too_short_secret() {
        // Secret trop court (< 30 chars)
        let body = "12345678:SHORTTOKEN";
        let token = extract_token_from_body(body);
        assert!(token.is_none());
    }

    #[test]
    fn extract_token_from_body_no_token() {
        let body = "Pas de token ici";
        let token = extract_token_from_body(body);
        assert!(token.is_none());
    }

    #[test]
    fn extract_token_from_body_digits_too_short() {
        // Partie digits < 8 chars
        let body = "123:AABBCCDDEE_FF-GGHHIIJJKKLLMMNNOOPPQQRRSSttuu";
        let token = extract_token_from_body(body);
        assert!(token.is_none());
    }

    // ── is_from_allowed ────────────────────────────────────────────────────────

    #[test]
    fn is_from_allowed_exact_match() {
        let allowed = vec!["motreff@gmail.com".to_string()];
        assert!(is_from_allowed("motreff@gmail.com", &allowed));
    }

    #[test]
    fn is_from_allowed_display_name() {
        // Gmail peut envoyer "Stéphane Motreffs <motreff@gmail.com>"
        let allowed = vec!["motreff@gmail.com".to_string()];
        assert!(is_from_allowed(
            "\"Stéphane Motreffs\" <motreff@gmail.com>",
            &allowed
        ));
    }

    #[test]
    fn is_from_allowed_case_insensitive() {
        let allowed = vec!["motreff@gmail.com".to_string()];
        assert!(is_from_allowed("MOTREFF@GMAIL.COM", &allowed));
    }

    #[test]
    fn is_from_allowed_rejected() {
        let allowed = vec!["motreff@gmail.com".to_string()];
        assert!(!is_from_allowed("attacker@evil.com", &allowed));
        assert!(!is_from_allowed("random@gmail.com", &allowed));
    }

    #[test]
    fn is_from_allowed_empty_allowlist() {
        // Allowlist vide → tout est rejeté
        let allowed: Vec<String> = vec![];
        assert!(!is_from_allowed("motreff@gmail.com", &allowed));
    }

    #[test]
    fn is_from_allowed_rejects_substring_spoofing() {
        let allowed = vec!["motreff@gmail.com".to_string()];
        assert!(!is_from_allowed("motreff@gmail.com.evil.com", &allowed));
        assert!(!is_from_allowed("attacker@motreff@gmail.com.evil.com", &allowed));
    }

    #[test]
    fn is_from_allowed_accepts_display_name_angle_bracket() {
        let allowed = vec!["motreff@gmail.com".to_string()];
        assert!(is_from_allowed("Stéphane <motreff@gmail.com>", &allowed));
        assert!(is_from_allowed(
            "\"Stéphane Motreff\" <motreff@gmail.com>",
            &allowed
        ));
    }

    #[test]
    fn extract_email_addr_handles_edge_cases() {
        assert_eq!(extract_email_addr("motreff@gmail.com"), "motreff@gmail.com");
        assert_eq!(extract_email_addr("<motreff@gmail.com>"), "motreff@gmail.com");
        assert_eq!(
            extract_email_addr("Name <motreff@gmail.com>"),
            "motreff@gmail.com"
        );
        assert_eq!(
            extract_email_addr("  motreff@gmail.com  "),
            "motreff@gmail.com"
        );
    }

    #[test]
    fn extract_admin_subject_rejects_uppercase() {
        assert_eq!(extract_admin_subject("HUBMQ_BOT_TOKEN LLMcore"), None);
        assert_eq!(extract_admin_subject("HUBMQ_BOT_TOKEN Claude"), None);
        assert_eq!(
            extract_admin_subject("HUBMQ_BOT_TOKEN llmcore"),
            Some("llmcore".to_string())
        );
    }

    // ── decode_quoted_printable ────────────────────────────────────────────────

    #[test]
    fn decode_qp_basic() {
        // `=3D` → `=`
        let input = "Bonjour=3DWorld";
        let decoded = decode_quoted_printable(input);
        assert_eq!(decoded, "Bonjour=World");
    }

    #[test]
    fn decode_qp_soft_line_wrap() {
        // `=\n` → rien (soft wrap)
        let input = "Hello=\nWorld";
        let decoded = decode_quoted_printable(input);
        assert_eq!(decoded, "HelloWorld");
    }

    // ── extract_plain_text_from_multipart ─────────────────────────────────────

    #[test]
    fn extract_plain_from_multipart_basic() {
        let body = "--boundary123\r\nContent-Type: text/plain\r\n\r\nHello from plain!\r\n--boundary123\r\nContent-Type: text/html\r\n\r\n<html></html>\r\n--boundary123--";
        let content_type = "multipart/alternative; boundary=boundary123";
        let result = extract_plain_text_from_multipart(body, content_type);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Hello from plain!"));
        assert!(!text.contains("<html>"));
    }
}
