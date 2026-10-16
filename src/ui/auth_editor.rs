//! Editor de autorización (Bearer, Basic, API Key, OAuth 2.0).

use super::*;

/// Qué pidió el editor de auth.
#[derive(Default)]
pub(super) struct AuthUi {
    pub(super) get_token: bool,
    pub(super) edit_collection: bool,
}

pub(super) fn auth_label(a: &model::Auth) -> &'static str {
    match a {
        model::Auth::Inherit => t("Heredar de la colección", "Inherit from collection"),
        model::Auth::None => t("Sin auth", "No auth"),
        model::Auth::Bearer { .. } => "Bearer token",
        model::Auth::Basic { .. } => "Basic auth",
        model::Auth::ApiKey { .. } => "API Key",
        model::Auth::Oauth2(_) => "OAuth 2.0",
    }
}

/// Campo con etiqueta de ancho fijo. Los secretos se ocultan.
pub(super) fn auth_field(ui: &mut egui::Ui, label: &str, value: &mut String, secret: bool, hint: &str) -> bool {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(130.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(130.0);
            ui.label(RichText::new(label).color(theme::muted()));
        });
        let mut lay = widgets::line_layouter(Lang::Plain, 13.5);
        let mut edit = TextEdit::singleline(value).hint_text(hint).font(TextStyle::Monospace).desired_width(ui.available_width().min(560.0));
        edit = if secret { edit.password(true) } else { edit.layouter(&mut lay) };
        ui.add(edit).changed()
    })
    .inner
}

/// Editor de autorización (request o colección). `inherited` = lo que heredaría un request.
pub(super) fn auth_editor(ui: &mut egui::Ui, auth: &mut model::Auth, in_collection: bool, inherited: Option<(&str, &model::Auth)>, fetching: bool) -> (bool, AuthUi) {
    use model::Auth;
    let mut changed = false;
    let mut out = AuthUi::default();
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(130.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(130.0);
            ui.label(RichText::new(t("Tipo", "Type")).color(theme::muted()));
        });
        let mut options = vec![Auth::None, Auth::Bearer { token: String::new() }, Auth::Basic { user: String::new(), pass: String::new() }];
        options.push(Auth::ApiKey { key: "X-API-Key".into(), value: String::new(), in_query: false });
        options.push(Auth::Oauth2(model::OAuth2 { grant: "client_credentials".into(), callback_url: auth::DEFAULT_CALLBACK.into(), ..Default::default() }));
        if !in_collection {
            options.insert(0, Auth::Inherit);
        }
        egui::ComboBox::from_id_salt("auth_type").width(240.0).selected_text(auth_label(auth)).show_ui(ui, |ui| {
            for o in options {
                let same = std::mem::discriminant(&o) == std::mem::discriminant(auth);
                if ui.selectable_label(same, auth_label(&o)).clicked() && !same {
                    *auth = o;
                    changed = true;
                }
            }
        });
    });
    ui.add_space(6.0);
    match auth {
        Auth::Inherit => {
            let (col, a) = inherited.unwrap_or((t("(sin colección)", "(no collection)"), &Auth::None));
            ui.label(RichText::new(tr!("Usa la autorización de la colección «{col}»: {}.", "Uses the authorization of collection «{col}»: {}.", auth_label(a))).color(theme::muted()));
            if ui.button(t("Editar la autorización de la colección", "Edit the collection's authorization")).clicked() {
                out.edit_collection = true;
            }
        }
        Auth::None => {
            ui.label(RichText::new(if in_collection { t("Los requests que hereden no llevarán autorización.", "Requests that inherit will be sent without authorization.") } else { t("Este request se envía sin autorización.", "This request is sent without authorization.") }).color(theme::muted()));
        }
        Auth::Bearer { token } => {
            changed |= auth_field(ui, "Token", token, false, "{{token}}");
            ui.label(RichText::new(t("Se envía como Authorization: Bearer <token>.", "Sent as Authorization: Bearer <token>.")).small().color(theme::muted()));
        }
        Auth::Basic { user, pass } => {
            changed |= auth_field(ui, t("Usuario", "User"), user, false, t("{{usuario}}", "{{user}}"));
            changed |= auth_field(ui, t("Contraseña", "Password"), pass, true, "");
        }
        Auth::ApiKey { key, value, in_query } => {
            changed |= auth_field(ui, t("Clave", "Key"), key, false, "X-API-Key");
            changed |= auth_field(ui, t("Valor", "Value"), value, false, "{{apiKey}}");
            ui.horizontal(|ui| {
                ui.add_space(138.0);
                changed |= ui.radio_value(in_query, false, t("En un header", "In a header")).changed();
                changed |= ui.radio_value(in_query, true, t("En la query de la URL", "In the URL query")).changed();
            });
        }
        Auth::Oauth2(o) => {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(130.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_width(130.0);
                    ui.label(RichText::new("Grant type").color(theme::muted()));
                });
                let shown = auth::grant_label(&o.grant);
                egui::ComboBox::from_id_salt("grant").width(240.0).selected_text(shown).show_ui(ui, |ui| {
                    for (k, _) in auth::GRANTS {
                        changed |= ui.selectable_value(&mut o.grant, k.to_string(), auth::grant_label(k)).changed();
                    }
                });
            });
            let code = o.grant == "authorization_code";
            if code {
                changed |= auth_field(ui, "Auth URL", &mut o.auth_url, false, t("https://login.ejemplo.com/oauth2/authorize", "https://login.example.com/oauth2/authorize"));
            }
            changed |= auth_field(ui, "Token URL", &mut o.token_url, false, t("https://login.ejemplo.com/oauth2/token", "https://login.example.com/oauth2/token"));
            if code {
                changed |= auth_field(ui, "Callback URL", &mut o.callback_url, false, auth::DEFAULT_CALLBACK);
            }
            changed |= auth_field(ui, "Client ID", &mut o.client_id, false, "{{clientId}}");
            changed |= auth_field(ui, "Client secret", &mut o.client_secret, true, t("vacío si es un cliente público", "empty for a public client"));
            changed |= auth_field(ui, "Scope", &mut o.scope, false, "api.read api.write");
            if o.grant == "password" {
                changed |= auth_field(ui, t("Usuario", "User"), &mut o.username, false, t("{{usuario}}", "{{user}}"));
                changed |= auth_field(ui, t("Contraseña", "Password"), &mut o.password, true, "");
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(138.0);
                let now = auth::now();
                if o.access_token.is_empty() {
                    widgets::pill(ui, t("sin token", "no token"), theme::muted());
                } else if o.expires_at == 0 {
                    widgets::pill(ui, t("token sin vencimiento conocido", "token with no known expiry"), theme::ok());
                } else if o.expires_at > now {
                    widgets::pill(ui, tr!("vigente {} min", "valid {} min", (o.expires_at - now) / 60), theme::ok());
                } else {
                    widgets::pill(ui, t("vencido", "expired"), theme::warn());
                }
                let label = if fetching { t("Obteniendo…", "Fetching…") } else { t("Obtener token", "Get token") };
                if ui.add_enabled(!fetching, Button::new(RichText::new(label).strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                    out.get_token = true;
                }
                if !o.access_token.is_empty() && ui.button(t("Borrar token", "Clear token")).clicked() {
                    (o.access_token, o.refresh_token, o.expires_at) = (String::new(), String::new(), 0);
                    changed = true;
                }
            });
            let note = if code {
                t("Abre tu navegador para iniciar sesión (PKCE). Registra la Callback URL en el proveedor. Con refresh token se renueva solo al enviar.", "Opens your browser to sign in (PKCE). Register the Callback URL with the provider. With a refresh token it renews automatically when sending.")
            } else {
                t("Al enviar se pide o renueva el token solo cuando hace falta (también en el Runner y la consola).", "When sending, the token is requested or renewed only when needed (also in the Runner and the CLI).")
            };
            ui.label(RichText::new(note).small().color(theme::muted()));
        }
    }
    (changed, out)
}
