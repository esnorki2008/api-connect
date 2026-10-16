//! OpenAPI 3.x / Swagger 2.0 (YAML o JSON) -> colección de requests.
//! Cada endpoint queda con `origin.key = "MÉTODO /ruta"` para poder sincronizarlo después.

use serde_json::{Map, Value, json};

use crate::{
    domain::model::{Collection, Origin, Request, Vars},
    i18n::t,
    tr,
};

const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

/// YAML es superconjunto de JSON: un solo parser sirve para los dos formatos.
pub fn parse(text: &str) -> Result<Value, String> {
    serde_yaml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| tr!("YAML/JSON inválido: {e}", "invalid YAML/JSON: {e}"))
}

pub fn is_openapi(v: &Value) -> bool {
    v.get("openapi").is_some() || v.get("swagger").is_some()
}

pub fn to_collection(spec: &Value, fallback_name: &str) -> Result<Collection, String> {
    let paths = spec["paths"].as_object().ok_or(t("El OpenAPI no tiene 'paths'", "The OpenAPI has no 'paths'"))?;
    let mut vars = Vars::new();
    vars.insert("baseUrl".into(), base_url(spec));
    let mut requests = Vec::new();
    for (path, item) in paths {
        let item = deref(spec, item);
        for method in METHODS {
            if let Some(op) = item.get(method) {
                requests.push(endpoint(spec, path, method, &item, op, &mut vars));
            }
        }
    }
    requests.sort_by(|a, b| a.name.cmp(&b.name)); // agrupa por tag, que va como prefijo del nombre
    let name = spec["info"]["title"].as_str().filter(|t| !t.trim().is_empty()).unwrap_or(fallback_name);
    Ok(Collection { name: name.to_string(), variables: vars, requests, ..Default::default() })
}

fn base_url(spec: &Value) -> String {
    if spec.get("swagger").is_some() {
        let scheme = spec["schemes"][0].as_str().unwrap_or("https");
        let host = spec["host"].as_str().unwrap_or("localhost");
        return format!("{scheme}://{host}{}", spec["basePath"].as_str().unwrap_or("")).trim_end_matches('/').to_string();
    }
    let server = &spec["servers"][0];
    let mut url = server["url"].as_str().unwrap_or("http://localhost").to_string();
    if let Some(vars) = server["variables"].as_object() {
        for (k, v) in vars {
            url = url.replace(&format!("{{{k}}}"), &text(&v["default"]));
        }
    }
    url.trim_end_matches('/').to_string()
}

/// Sigue un "$ref" local ("#/components/schemas/X"). Los ciclos los corta el límite de profundidad.
fn deref(spec: &Value, v: &Value) -> Value {
    let mut cur = v.clone();
    for _ in 0..20 {
        match cur["$ref"].as_str().and_then(|r| r.strip_prefix('#')).and_then(|p| spec.pointer(p)) {
            Some(target) => cur = target.clone(),
            None => break,
        }
    }
    cur
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Schema con los $ref sustituidos, para guardarlo junto al endpoint y validar sin el spec completo.
/// ponytail: profundidad 4 de $ref; schemas más anidados se validan solo hasta ahí.
fn inline(spec: &Value, v: &Value, depth: u32) -> Value {
    match v {
        Value::Object(o) if o.contains_key("$ref") => {
            if depth >= 4 { json!({}) } else { inline(spec, &deref(spec, v), depth + 1) }
        }
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), inline(spec, x, depth))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|x| inline(spec, x, depth)).collect()),
        other => other.clone(),
    }
}

/// Valor de ejemplo para un schema: example/default/enum o uno generado según el tipo.
fn example(spec: &Value, schema: &Value, depth: u32) -> Value {
    if depth > 8 {
        return Value::Null;
    }
    let s = deref(spec, schema);
    for k in ["example", "default"] {
        if let Some(v) = s.get(k) {
            return v.clone();
        }
    }
    if let Some(v) = s["enum"].as_array().and_then(|a| a.first()) {
        return v.clone();
    }
    if let Some(parts) = s["allOf"].as_array() {
        let mut obj = Map::new();
        for p in parts {
            if let Value::Object(o) = example(spec, p, depth + 1) {
                obj.extend(o);
            }
        }
        return Value::Object(obj);
    }
    for k in ["oneOf", "anyOf"] {
        if let Some(first) = s[k].as_array().and_then(|a| a.first()) {
            return example(spec, first, depth + 1);
        }
    }
    // OpenAPI 3.1 permite type: [string, "null"]
    let ty = match &s["type"] {
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or(""),
        t => t.as_str().unwrap_or(""),
    };
    match ty {
        "array" => json!([example(spec, &s["items"], depth + 1)]),
        "integer" | "number" => json!(0),
        "boolean" => json!(true),
        "string" => json!(match s["format"].as_str().unwrap_or("") {
            "date-time" => "2024-01-01T00:00:00Z",
            "date" => "2024-01-01",
            "uuid" => "00000000-0000-0000-0000-000000000000",
            "email" => "user@example.com",
            _ => "string",
        }),
        _ => match s["properties"].as_object() {
            Some(props) => Value::Object(props.iter().map(|(k, v)| (k.clone(), example(spec, v, depth + 1))).collect()),
            None if ty == "object" => json!({}),
            None => Value::Null,
        },
    }
}

/// Ejemplo de un media type (OpenAPI 3): example, primer examples.*.value o generado del schema.
fn media_example(spec: &Value, media: &Value) -> Value {
    if let Some(e) = media.get("example") {
        return e.clone();
    }
    if let Some(first) = media["examples"].as_object().and_then(|m| m.values().next()) {
        let first = deref(spec, first);
        if let Some(v) = first.get("value") {
            return v.clone();
        }
    }
    example(spec, &media["schema"], 0)
}

fn param_example(spec: &Value, p: &Value) -> String {
    // Swagger 2 pone type/default/enum en el propio parámetro; OpenAPI 3 dentro de "schema".
    let schema = if p["schema"].is_null() { p } else { &p["schema"] };
    match p.get("example") {
        Some(e) => text(e),
        None => match example(spec, schema, 0) {
            Value::Null => String::new(),
            v => text(&v),
        },
    }
}

fn auth_header(spec: &Value, op: &Value, vars: &mut Vars) -> Option<String> {
    let reqs = op.get("security").or_else(|| spec.get("security"))?.as_array()?; // [] = sin auth
    let name = reqs.first()?.as_object()?.keys().next()?;
    let schemes = if spec.get("swagger").is_some() { &spec["securityDefinitions"] } else { &spec["components"]["securitySchemes"] };
    let s = deref(spec, &schemes[name]);
    let (header, var) = match (s["type"].as_str()?, s["scheme"].as_str().unwrap_or("").to_lowercase().as_str()) {
        ("http", "basic") | ("basic", _) => ("Authorization: Basic {{basicAuth}}".to_string(), "basicAuth"),
        ("apiKey", _) if s["in"] == "header" => (format!("{}: {{{{apiKey}}}}", s["name"].as_str()?), "apiKey"),
        // ponytail: apiKey en query/cookie no se genera; agrégalo a mano en la URL si tu API lo usa.
        ("apiKey", _) => return None,
        _ => ("Authorization: Bearer {{token}}".to_string(), "token"), // http bearer, oauth2, openIdConnect
    };
    // Vacía en la colección: un valor del entorno con el mismo nombre tiene prioridad.
    vars.entry(var.into()).or_default();
    Some(header)
}

fn endpoint(spec: &Value, path: &str, method: &str, item: &Value, op: &Value, vars: &mut Vars) -> Request {
    // Parámetros de la ruta + los de la operación; los de la operación pisan a los compartidos.
    let mut params: Vec<Value> = Vec::new();
    for p in item["parameters"].as_array().into_iter().chain(op["parameters"].as_array()).flatten() {
        let p = deref(spec, p);
        params.retain(|q| !(q["name"] == p["name"] && q["in"] == p["in"]));
        params.push(p);
    }

    let mut url = format!("{{{{baseUrl}}}}{}", path.replace('{', "{{").replace('}', "}}"));
    let mut query = Vec::new();
    let mut headers = Vec::new();
    for p in &params {
        let name = p["name"].as_str().unwrap_or_default();
        let required = p["required"].as_bool().unwrap_or(false);
        match p["in"].as_str().unwrap_or_default() {
            "path" => {
                vars.entry(name.into()).or_insert_with(|| param_example(spec, p));
            }
            "query" if required => query.push(format!("{name}={{{{{name}}}}}")),
            "header" if required => headers.push(format!("{name}: {{{{{name}}}}}")),
            _ => continue,
        }
        if p["in"] != "path" {
            vars.entry(name.into()).or_insert_with(|| param_example(spec, p));
        }
    }
    if !query.is_empty() {
        url = format!("{url}?{}", query.join("&"));
    }
    if let Some(h) = auth_header(spec, op, vars) {
        headers.push(h);
    }

    let mut body = String::new();
    let mut schema = Value::Null;
    let rb = deref(spec, &op["requestBody"]);
    if let Some(content) = rb["content"].as_object() {
        // OpenAPI 3: preferir JSON, luego formulario, luego lo que haya.
        let ct = ["application/json", "application/x-www-form-urlencoded"]
            .into_iter()
            .find(|c| content.contains_key(*c))
            .or_else(|| content.keys().find(|k| k.contains("json")).map(String::as_str))
            .or_else(|| content.keys().next().map(String::as_str));
        if let Some(ct) = ct {
            let ex = media_example(spec, &content[ct]);
            body = form_or_json(ct, &ex);
            headers.push(format!("Content-Type: {ct}"));
            if ct.contains("json") {
                schema = inline(spec, &content[ct]["schema"], 0);
            }
        }
    } else if let Some(p) = params.iter().find(|p| p["in"] == "body") {
        // Swagger 2: parámetro "in: body"
        body = form_or_json("application/json", &example(spec, &p["schema"], 0));
        headers.push("Content-Type: application/json".into());
        schema = inline(spec, &p["schema"], 0);
    } else {
        let form: Vec<String> = params
            .iter()
            .filter(|p| p["in"] == "formData" && p["type"] != "file")
            .map(|p| format!("{}={}", text(&p["name"]), param_example(spec, p)))
            .collect();
        if !form.is_empty() {
            body = form.join("&");
            headers.push("Content-Type: application/x-www-form-urlencoded".into());
        }
    }

    let key = format!("{} {path}", method.to_uppercase());
    let title = op["summary"].as_str().or(op["operationId"].as_str()).filter(|s| !s.trim().is_empty()).unwrap_or(&key).to_string();
    let name = match op["tags"][0].as_str() {
        Some(tag) => format!("{tag} / {title}"),
        None => title,
    };
    let mut r = Request { name, method: method.to_uppercase(), url, headers: headers.join("\n"), body, ..Default::default() };
    r.origin = Some(Origin { key, base: r.parts(), schema });
    r
}

fn form_or_json(ct: &str, ex: &Value) -> String {
    match (ct, ex) {
        (_, Value::Null) => String::new(),
        ("application/x-www-form-urlencoded", Value::Object(o)) => {
            o.iter().map(|(k, v)| format!("{k}={}", text(v))).collect::<Vec<_>>().join("&")
        }
        (_, Value::String(s)) => s.clone(),
        _ => serde_json::to_string_pretty(ex).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"
openapi: 3.0.3
info: { title: Tienda, version: "1" }
servers:
  - url: https://{env}.tienda.com/api/
    variables: { env: { default: qa } }
security: [ { bearer: [] } ]
components:
  securitySchemes:
    bearer: { type: http, scheme: bearer }
    key: { type: apiKey, in: header, name: X-Api-Key }
  parameters:
    Id: { name: id, in: path, required: true, schema: { type: integer, example: 7 } }
  schemas:
    Product:
      type: object
      properties:
        id: { type: integer }
        name: { type: string, example: Silla }
        tags: { type: array, items: { type: string } }
        parent: { $ref: '#/components/schemas/Product' }
paths:
  /products/{id}:
    parameters: [ { $ref: '#/components/parameters/Id' } ]
    get:
      tags: [productos]
      summary: Obtener producto
      parameters:
        - { name: expand, in: query, required: true, schema: { type: string, enum: [all, none] } }
        - { name: debug, in: query, schema: { type: boolean } }
    put:
      tags: [productos]
      operationId: updateProduct
      security: [ { key: [] } ]
      requestBody:
        content:
          application/json: { schema: { $ref: '#/components/schemas/Product' } }
  /health:
    get: { security: [] }
"#;

    #[test]
    fn converts_openapi3() {
        let c = to_collection(&parse(SPEC).unwrap(), "x").unwrap();
        assert_eq!(c.name, "Tienda");
        assert_eq!(c.variables.get("baseUrl").unwrap(), "https://qa.tienda.com/api");
        assert_eq!(c.variables.get("id").unwrap(), "7");
        assert_eq!(c.variables.get("expand").unwrap(), "all");
        assert!(c.variables.contains_key("token") && c.variables.contains_key("apiKey"));
        assert_eq!(c.requests.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["GET /health", "productos / Obtener producto", "productos / updateProduct"]);

        let (health, get, put) = (&c.requests[0], &c.requests[1], &c.requests[2]);
        assert_eq!((health.url.as_str(), health.headers.as_str()), ("{{baseUrl}}/health", ""));
        assert_eq!(get.url, "{{baseUrl}}/products/{{id}}?expand={{expand}}");
        assert_eq!(get.headers, "Authorization: Bearer {{token}}");
        assert_eq!(get.origin.as_ref().unwrap().key, "GET /products/{id}");
        assert_eq!(put.headers, "X-Api-Key: {{apiKey}}\nContent-Type: application/json");
        let body: Value = serde_json::from_str(&put.body).unwrap();
        assert_eq!(body["name"], "Silla");
        assert_eq!(body["tags"], json!(["string"]));
        assert!(body["parent"].is_object(), "el $ref recursivo se corta por profundidad sin colgarse");
        assert!(!put.modified());
        let schema = &put.origin.as_ref().unwrap().schema;
        assert_eq!(schema["properties"]["name"]["type"], "string");
        assert!(crate::domain::overlay::validate(&put.body, schema).is_empty(), "el ejemplo generado cumple su propio schema");
    }

    #[test]
    fn converts_swagger2() {
        let spec = parse(
            r##"{"swagger":"2.0","info":{"title":"Legacy"},"host":"api.x.com","basePath":"/v1","schemes":["http"],
              "definitions":{"U":{"type":"object","properties":{"n":{"type":"string"}}}},
              "paths":{"/u":{"post":{"parameters":[{"in":"body","name":"b","schema":{"$ref":"#/definitions/U"}}]}},
                       "/f":{"post":{"parameters":[{"in":"formData","name":"a","type":"string","default":"z"}]}}}}"##,
        )
        .unwrap();
        let c = to_collection(&spec, "x").unwrap();
        assert_eq!(c.variables.get("baseUrl").unwrap(), "http://api.x.com/v1");
        assert_eq!(c.requests[1].body, "{\n  \"n\": \"string\"\n}");
        assert_eq!((c.requests[0].body.as_str(), c.requests[0].headers.as_str()), ("a=z", "Content-Type: application/x-www-form-urlencoded"));
    }
}
