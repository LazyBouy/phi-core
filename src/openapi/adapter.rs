use super::types::*;
use crate::types::{AgentTool, Content, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use openapiv3::{
    OpenAPI, Operation, Parameter, ParameterSchemaOrContent, ReferenceOr, RequestBody, Schema,
};
use std::sync::Arc;

/// Wraps a single OpenAPI operation as an `AgentTool`.
///
/// Created via factory methods that parse an OpenAPI spec and produce
/// one adapter per operation. Each adapter makes HTTP requests to the
/// API endpoint when executed.
#[derive(Debug)]
pub struct OpenApiToolAdapter {
    client: Arc<reqwest::Client>,
    config: OpenApiConfig,
    base_url: String,
    info: OperationInfo,
    /// Pre-formatted tool name (with optional prefix).
    tool_name: String,
}

impl OpenApiToolAdapter {
    /// Parse an OpenAPI spec from a string (JSON or YAML) and create tool adapters.
    pub fn from_str(
        spec_str: &str, // RAW SPEC — JSON or YAML string; format auto-detected by first character
        config: OpenApiConfig, // ADAPTER SETTINGS — auth, base_url override, timeout, name prefix
        filter: &OperationFilter, // SELECTOR — which operations become tools (All, ByTag, ByOperationId, ByPathPrefix)
    ) -> Result<Vec<Self>, OpenApiError> {
        let spec = parse_spec(spec_str)?;
        Self::from_spec(spec, config, filter)
    }

    /// Read an OpenAPI spec from a file and create tool adapters.
    pub async fn from_file(
        path: impl AsRef<std::path::Path>, // FILE PATH — read as text then passed to from_str (JSON or YAML)
        config: OpenApiConfig, // ADAPTER SETTINGS — auth, base_url override, timeout, name prefix
        filter: &OperationFilter, // SELECTOR — which operations become tools
    ) -> Result<Vec<Self>, OpenApiError> {
        let content = tokio::fs::read_to_string(path).await?;
        Self::from_str(&content, config, filter)
    }

    /// Fetch an OpenAPI spec from a URL and create tool adapters.
    pub async fn from_url(
        url: &str, // SPEC URL — fetched via HTTP GET then passed to from_str (JSON or YAML)
        config: OpenApiConfig, // ADAPTER SETTINGS — auth, base_url override, timeout, name prefix
        filter: &OperationFilter, // SELECTOR — which operations become tools
    ) -> Result<Vec<Self>, OpenApiError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(OpenApiError::HttpError)?;
        let resp = client.get(url).send().await?.text().await?;
        Self::from_str(&resp, config, filter)
    }

    /// Create tool adapters from a parsed OpenAPI spec.
    /*
    DESIGN: Why `spec`, `config`, and `filter` are separate parameters
      `spec`   = PARSED CONTENT  — the OpenAPI document: paths, operations, schemas, $ref targets
      `config` = ADAPTER SETTINGS — how to call the API: auth, base_url override, timeout, name prefix
      `filter` = OPERATION SUBSET — which operations to expose as tools; decoupled from both spec and config
    Three independent concerns: what the API does vs how to call it vs which parts to expose.
    Separating them lets callers share one config across multiple specs, or vary the filter without
    rebuilding the whole adapter.
    */
    pub fn from_spec(
        spec: OpenAPI, // PARSED CONTENT — the full OpenAPI 3.0 document (paths, operations, $ref targets)
        config: OpenApiConfig, // ADAPTER SETTINGS — auth, base_url override, timeout, name prefix
        filter: &OperationFilter, // SELECTOR — which operations become tools (All, ByTag, ByOperationId, ByPathPrefix)
    ) -> Result<Vec<Self>, OpenApiError> {
        let base_url = config
            .base_url
            .clone()
            .or_else(|| {
                spec.servers
                    .first()
                    .map(|s| s.url.trim_end_matches('/').to_string())
            })
            .ok_or(OpenApiError::NoBaseUrl)?;

        let client = Arc::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(config.timeout_secs))
                .build()
                .map_err(OpenApiError::HttpError)?,
        );

        let mut adapters = Vec::new();

        for (path, method, operation) in spec.operations() {
            let operation_id = match &operation.operation_id {
                Some(id) => id.clone(),
                None => {
                    tracing::warn!(
                        path = path,
                        method = method,
                        "Skipping operation without operationId"
                    );
                    continue;
                }
            };

            let tags: Vec<&str> = operation.tags.iter().map(|s| s.as_str()).collect();

            if !matches_filter(&operation_id, &tags, path, filter) {
                continue;
            }

            let info = build_operation_info(&spec, &operation_id, method, path, operation)?;

            let tool_name = match &config.name_prefix {
                Some(prefix) => format!("{}__{}", prefix, operation_id),
                None => operation_id.clone(),
            };

            adapters.push(OpenApiToolAdapter {
                client: client.clone(),
                config: config.clone(),
                base_url: base_url.clone(),
                info,
                tool_name,
            });
        }

        Ok(adapters)
    }
}

#[async_trait]
impl AgentTool for OpenApiToolAdapter {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        self.info
            .summary
            .as_deref()
            .unwrap_or(&self.info.operation_id)
    }

    fn description(&self) -> &str {
        self.info
            .description
            .as_deref()
            .or(self.info.summary.as_deref())
            .unwrap_or_else(|| &self.info.path)
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.info.parameters_schema.clone()
    }

    async fn execute(
        &self,
        params: serde_json::Value, // LLM INPUT — JSON object with path/query/header/body params as top-level keys
        _ctx: ToolContext, // SYSTEM ENV — prefixed _ because HTTP tool manages its own timeout via reqwest
    ) -> Result<ToolResult, ToolError> {
        // Reject non-object params — return as content so LLM can self-correct
        let params = match params {
            serde_json::Value::Object(map) => map,
            serde_json::Value::Null => serde_json::Map::new(),
            other => {
                let type_name = match &other {
                    serde_json::Value::Array(_) => "array",
                    serde_json::Value::String(_) => "string",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::Bool(_) => "boolean",
                    _ => "non-object",
                };
                return Ok(ToolResult {
                    content: vec![Content::Text {
                        text: format!("Error: Expected object parameters, got {}", type_name),
                    }],
                    details: serde_json::json!({ "error": "invalid_args" }),
                });
            }
        };

        // Build URL with path parameters (URL-encode, validate present)
        let mut url_path = self.info.path.clone();
        for name in &self.info.path_params {
            let val = match params.get(name) {
                Some(v) => v,
                None => {
                    return Ok(ToolResult {
                        content: vec![Content::Text {
                            text: format!(
                                "Error: Missing required path parameter '{}' for {}",
                                name, self.info.path
                            ),
                        }],
                        details: serde_json::json!({ "error": "missing_path_param" }),
                    });
                }
            };
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            // URL-encode the value to prevent path traversal / injection
            let encoded = percent_encode_path_segment(&val_str);
            url_path = url_path.replace(&format!("{{{}}}", name), &encoded);
        }
        let url = format!("{}{}", self.base_url, url_path);

        // Build request
        let method = match self.info.method.parse::<reqwest::Method>() {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolResult {
                    content: vec![Content::Text {
                        text: format!("Error: Invalid HTTP method: {}", e),
                    }],
                    details: serde_json::json!({ "error": "invalid_method" }),
                });
            }
        };

        let mut req = self.client.request(method.clone(), &url);

        // Query parameters
        for name in &self.info.query_params {
            if let Some(val) = params.get(name) {
                let val_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                req = req.query(&[(name, val_str)]);
            }
        }

        // Header parameters
        for name in &self.info.header_params {
            if let Some(val) = params.get(name) {
                let val_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                req = req.header(name, val_str);
            }
        }

        // Auth
        match &self.config.auth {
            OpenApiAuth::None => {}
            OpenApiAuth::Bearer(token) => {
                req = req.bearer_auth(token);
            }
            OpenApiAuth::ApiKey { header, value } => {
                req = req.header(header, value);
            }
        }

        // Custom headers
        for (key, value) in &self.config.custom_headers {
            req = req.header(key, value);
        }

        // Body
        if self.info.has_body {
            let body_val = params.get("body").or_else(|| params.get("_request_body"));
            if let Some(body) = body_val {
                req = req.json(body);
            }
        }

        // Send — return errors as content so LLM can self-correct
        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult {
                    content: vec![Content::Text {
                        text: format!("Error: HTTP request failed: {}", e),
                    }],
                    details: serde_json::json!({
                        "error": "http_error",
                        "method": method.to_string(),
                        "url": url,
                    }),
                });
            }
        };

        let status = response.status();
        let mut body = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                return Ok(ToolResult {
                    content: vec![Content::Text {
                        text: format!(
                            "{} {} → {}\n\nError reading response: {}",
                            method, url, status, e
                        ),
                    }],
                    details: serde_json::json!({
                        "status": status.as_u16(),
                        "error": "read_error",
                        "method": method.to_string(),
                        "url": url,
                    }),
                });
            }
        };

        // UTF-8 safe truncation
        if body.len() > self.config.max_response_bytes {
            let mut end = self.config.max_response_bytes;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            body.truncate(end);
            body.push_str("\n... [truncated]");
        }

        let text = format!("{} {} → {}\n\n{}", method, url, status, body);

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({
                "status": status.as_u16(),
                "method": method.to_string(),
                "url": url,
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Percent-encode a path segment value, preserving unreserved characters.
fn percent_encode_path_segment(value: &str) -> String {
    // Encode everything except unreserved characters (RFC 3986 section 2.3)
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{:02X}", byte));
            }
        }
    }
    encoded
}

/// Issue #6: detect format first to provide accurate error messages.
fn parse_spec(input: &str) -> Result<OpenAPI, OpenApiError> {
    let trimmed = input.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str::<OpenAPI>(input)
            .map_err(|e| OpenApiError::ParseError(format!("JSON: {}", e)))
    } else {
        serde_yaml::from_str::<OpenAPI>(input)
            .map_err(|e| OpenApiError::ParseError(format!("YAML: {}", e)))
    }
}

fn matches_filter(
    operation_id: &str, // IDENTITY — the operation's operationId string (e.g. "listPets")
    tags: &[&str],      // CATEGORIES — operation tags (e.g. ["pets", "admin"])
    path: &str,         // ROUTE — the URL path (e.g. "/pets/{petId}")
    filter: &OperationFilter, // RULE — determines which combination of the above qualifies
) -> bool {
    match filter {
        OperationFilter::All => true,
        OperationFilter::ByOperationId(ids) => ids.iter().any(|id| id == operation_id),
        OperationFilter::ByTag(filter_tags) => {
            tags.iter().any(|t| filter_tags.iter().any(|ft| ft == t))
        }
        OperationFilter::ByPathPrefix(prefix) => path.starts_with(prefix.as_str()),
    }
}

/*
DESIGN: Why `spec` AND `operation` are separate parameters
  `spec`      = FULL DOCUMENT — needed for $ref resolution; parameters, schemas, and requestBodies
                can all be defined in `components/` and referenced via "$ref" pointers.
  `operation` = SINGLE OPERATION — the parsed struct for one HTTP method+path combination.
The operation itself cannot resolve its own $refs — it only contains the reference strings.
Only the full `spec` has the `components` section needed to look them up.
Keeping them separate mirrors the OpenAPI data model: the operation is a leaf,
the spec is the root that carries all shared definitions.
*/
fn build_operation_info(
    spec: &OpenAPI, // FULL DOCUMENT — needed to resolve $ref pointers in parameters and schemas
    operation_id: &str, // IDENTITY — the operationId string (stored as-is in OperationInfo)
    method: &str,   // HTTP METHOD — e.g. "get", "post"; uppercased before storage
    path: &str,     // ROUTE — URL path pattern with {placeholders} (e.g. "/pets/{petId}")
    operation: &Operation, // PARSED OP — the single operation's parameters, requestBody, summary, description
) -> Result<OperationInfo, OpenApiError> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    let mut path_params = Vec::new();
    let mut query_params = Vec::new();
    let mut header_params = Vec::new();

    // Process parameters
    for param_ref in &operation.parameters {
        let param = resolve_parameter(spec, param_ref)?;
        let data = param.parameter_data_ref();
        let name = &data.name;

        // Classify parameter
        match param {
            Parameter::Path { .. } => path_params.push(name.clone()),
            Parameter::Query { .. } => query_params.push(name.clone()),
            Parameter::Header { .. } => header_params.push(name.clone()),
            Parameter::Cookie { .. } => continue, // Skip cookies
        }

        // Issue #4: propagate schema extraction errors instead of swallowing
        let schema_json = extract_parameter_schema(spec, &data.format)?;
        properties.insert(name.clone(), schema_json.unwrap_or(serde_json::json!({})));

        if data.required {
            required.push(name.clone());
        }
    }

    // Issue #3: propagate request body resolution errors
    let has_body = if let Some(body_ref) = &operation.request_body {
        let body = resolve_request_body(spec, body_ref)?;
        if let Some(media) = body.content.get("application/json") {
            if let Some(schema_ref) = &media.schema {
                let schema_json = resolve_schema_to_json(spec, schema_ref)?;
                let body_key = if properties.contains_key("body") {
                    "_request_body".to_string()
                } else {
                    "body".to_string()
                };
                properties.insert(body_key.clone(), schema_json);
                if body.required {
                    required.push(body_key);
                }
            }
            true
        } else {
            false // Non-JSON body types (multipart, form-data) are unsupported
        }
    } else {
        false
    };

    let mut parameters_schema = serde_json::json!({
        "type": "object",
        "properties": properties,
    });
    if !required.is_empty() {
        parameters_schema["required"] = serde_json::json!(required);
    }

    Ok(OperationInfo {
        operation_id: operation_id.to_string(),
        method: method.to_uppercase(),
        path: path.to_string(),
        summary: operation.summary.clone(),
        description: operation.description.clone(),
        parameters_schema,
        path_params,
        query_params,
        header_params,
        has_body,
    })
}

fn resolve_parameter<'a>(
    spec: &'a OpenAPI, // FULL DOCUMENT — consulted when ref_or is a $ref (looks up components/parameters/)
    ref_or: &'a ReferenceOr<Parameter>, // ITEM OR REF — either an inline Parameter or a "$ref" string to resolve
) -> Result<&'a Parameter, OpenApiError> {
    match ref_or {
        ReferenceOr::Item(param) => Ok(param),
        ReferenceOr::Reference { reference } => {
            let name = reference
                .strip_prefix("#/components/parameters/")
                .ok_or_else(|| {
                    OpenApiError::InvalidSpec(format!("Unsupported parameter $ref: {}", reference))
                })?;
            let components = spec
                .components
                .as_ref()
                .ok_or_else(|| OpenApiError::InvalidSpec("No components section".into()))?;
            components
                .parameters
                .get(name)
                .and_then(|r| r.as_item())
                .ok_or_else(|| OpenApiError::InvalidSpec(format!("Parameter not found: {}", name)))
        }
    }
}

fn resolve_schema_to_json(
    spec: &OpenAPI, // FULL DOCUMENT — consulted when ref_or is a $ref (looks up components/schemas/)
    ref_or: &ReferenceOr<Schema>, // ITEM OR REF — either an inline Schema or a "$ref" string to resolve
) -> Result<serde_json::Value, OpenApiError> {
    match ref_or {
        ReferenceOr::Item(schema) => serde_json::to_value(schema).map_err(OpenApiError::JsonError),
        ReferenceOr::Reference { reference } => {
            let name = reference
                .strip_prefix("#/components/schemas/")
                .ok_or_else(|| {
                    OpenApiError::InvalidSpec(format!("Unsupported schema $ref: {}", reference))
                })?;
            let components = spec
                .components
                .as_ref()
                .ok_or_else(|| OpenApiError::InvalidSpec("No components section".into()))?;
            let schema = components
                .schemas
                .get(name)
                .and_then(|r| r.as_item())
                .ok_or_else(|| OpenApiError::InvalidSpec(format!("Schema not found: {}", name)))?;
            serde_json::to_value(schema).map_err(OpenApiError::JsonError)
        }
    }
}

/// Issue #3: return Result instead of Option to propagate $ref resolution failures.
fn resolve_request_body<'a>(
    spec: &'a OpenAPI, // FULL DOCUMENT — consulted when ref_or is a $ref (looks up components/requestBodies/)
    ref_or: &'a ReferenceOr<RequestBody>, // ITEM OR REF — either an inline RequestBody or a "$ref" string to resolve
) -> Result<&'a RequestBody, OpenApiError> {
    match ref_or {
        ReferenceOr::Item(body) => Ok(body),
        ReferenceOr::Reference { reference } => {
            let name = reference
                .strip_prefix("#/components/requestBodies/")
                .ok_or_else(|| {
                    OpenApiError::InvalidSpec(format!(
                        "Unsupported requestBody $ref: {}",
                        reference
                    ))
                })?;
            let components = spec
                .components
                .as_ref()
                .ok_or_else(|| OpenApiError::InvalidSpec("No components section".into()))?;
            components
                .request_bodies
                .get(name)
                .and_then(|r| r.as_item())
                .ok_or_else(|| {
                    OpenApiError::InvalidSpec(format!("Request body not found: {}", name))
                })
        }
    }
}

/// Issue #4: return Result to propagate schema resolution errors.
fn extract_parameter_schema(
    spec: &OpenAPI, // FULL DOCUMENT — passed to resolve_schema_to_json for $ref resolution
    format: &ParameterSchemaOrContent, // SCHEMA OR CONTENT — Schema variant → extract JSON schema; Content → unsupported, returns None
) -> Result<Option<serde_json::Value>, OpenApiError> {
    match format {
        ParameterSchemaOrContent::Schema(ref_or) => resolve_schema_to_json(spec, ref_or).map(Some),
        ParameterSchemaOrContent::Content(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PETSTORE_JSON: &str = r#"{
        "openapi": "3.0.0",
        "info": { "title": "Petstore", "version": "1.0.0" },
        "servers": [{ "url": "https://petstore.example.com/v1" }],
        "paths": {
            "/pets": {
                "get": {
                    "operationId": "listPets",
                    "summary": "List all pets",
                    "tags": ["pets"],
                    "parameters": [
                        {
                            "name": "limit",
                            "in": "query",
                            "required": false,
                            "schema": { "type": "integer" }
                        }
                    ],
                    "responses": { "200": { "description": "A list of pets" } }
                },
                "post": {
                    "operationId": "createPet",
                    "summary": "Create a pet",
                    "tags": ["pets"],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "tag": { "type": "string" }
                                    },
                                    "required": ["name"]
                                }
                            }
                        }
                    },
                    "responses": { "201": { "description": "Pet created" } }
                }
            },
            "/pets/{petId}": {
                "get": {
                    "operationId": "getPet",
                    "summary": "Get a pet by ID",
                    "tags": ["pets"],
                    "parameters": [
                        {
                            "name": "petId",
                            "in": "path",
                            "required": true,
                            "schema": { "type": "string" }
                        }
                    ],
                    "responses": { "200": { "description": "A pet" } }
                },
                "delete": {
                    "operationId": "deletePet",
                    "summary": "Delete a pet",
                    "tags": ["pets", "admin"],
                    "parameters": [
                        {
                            "name": "petId",
                            "in": "path",
                            "required": true,
                            "schema": { "type": "string" }
                        }
                    ],
                    "responses": { "204": { "description": "Pet deleted" } }
                }
            },
            "/users": {
                "get": {
                    "operationId": "listUsers",
                    "summary": "List users",
                    "tags": ["users"],
                    "responses": { "200": { "description": "A list of users" } }
                }
            }
        }
    }"#;

    const PETSTORE_YAML: &str = r#"
openapi: "3.0.0"
info:
  title: Petstore
  version: "1.0.0"
servers:
  - url: https://petstore.example.com/v1
paths:
  /pets:
    get:
      operationId: listPets
      summary: List all pets
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
      responses:
        "200":
          description: A list of pets
"#;

    #[test]
    fn test_parse_json_spec() {
        let adapters = OpenApiToolAdapter::from_str(
            PETSTORE_JSON,
            OpenApiConfig::default(),
            &OperationFilter::All,
        )
        .unwrap();
        assert_eq!(adapters.len(), 5);
    }

    #[test]
    fn test_parse_yaml_spec() {
        let adapters = OpenApiToolAdapter::from_str(
            PETSTORE_YAML,
            OpenApiConfig::default(),
            &OperationFilter::All,
        )
        .unwrap();
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].name(), "listPets");
    }

    #[test]
    fn test_operation_count() {
        let adapters = OpenApiToolAdapter::from_str(
            PETSTORE_JSON,
            OpenApiConfig::default(),
            &OperationFilter::All,
        )
        .unwrap();
        let names: Vec<&str> = adapters.iter().map(|a| a.name()).collect();
        assert!(names.contains(&"listPets"));
        assert!(names.contains(&"createPet"));
        assert!(names.contains(&"getPet"));
        assert!(names.contains(&"deletePet"));
        assert!(names.contains(&"listUsers"));
    }

    #[test]
    fn test_parameter_schema_query() {
        let adapters = OpenApiToolAdapter::from_str(
            PETSTORE_JSON,
            OpenApiConfig::default(),
            &OperationFilter::All,
        )
        .unwrap();
        let list_pets = adapters.iter().find(|a| a.name() == "listPets").unwrap();
        let schema = list_pets.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["limit"].is_object());
        // No required params → "required" key should be absent
        assert!(schema.get("required").is_none());
    }

    #[test]
    fn test_parameter_schema_path() {
        let adapters = OpenApiToolAdapter::from_str(
            PETSTORE_JSON,
            OpenApiConfig::default(),
            &OperationFilter::All,
        )
        .unwrap();
        let get_pet = adapters.iter().find(|a| a.name() == "getPet").unwrap();
        let schema = get_pet.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["petId"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("petId")));
    }

    #[test]
    fn test_parameter_schema_body() {
        let adapters = OpenApiToolAdapter::from_str(
            PETSTORE_JSON,
            OpenApiConfig::default(),
            &OperationFilter::All,
        )
        .unwrap();
        let create_pet = adapters.iter().find(|a| a.name() == "createPet").unwrap();
        let schema = create_pet.parameters_schema();
        assert!(schema["properties"]["body"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("body")));
    }

    #[test]
    fn test_filter_by_operation_id() {
        let filter = OperationFilter::ByOperationId(vec!["listPets".into(), "getPet".into()]);
        let adapters =
            OpenApiToolAdapter::from_str(PETSTORE_JSON, OpenApiConfig::default(), &filter).unwrap();
        assert_eq!(adapters.len(), 2);
    }

    #[test]
    fn test_filter_by_tag() {
        let filter = OperationFilter::ByTag(vec!["admin".into()]);
        let adapters =
            OpenApiToolAdapter::from_str(PETSTORE_JSON, OpenApiConfig::default(), &filter).unwrap();
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].name(), "deletePet");
    }

    #[test]
    fn test_filter_by_path_prefix() {
        let filter = OperationFilter::ByPathPrefix("/users".into());
        let adapters =
            OpenApiToolAdapter::from_str(PETSTORE_JSON, OpenApiConfig::default(), &filter).unwrap();
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].name(), "listUsers");
    }

    #[test]
    fn test_tool_trait_name_with_prefix() {
        let config = OpenApiConfig::default().with_name_prefix("petstore");
        let adapters =
            OpenApiToolAdapter::from_str(PETSTORE_JSON, config, &OperationFilter::All).unwrap();
        let names: Vec<&str> = adapters.iter().map(|a| a.name()).collect();
        assert!(names.contains(&"petstore__listPets"));
    }

    #[test]
    fn test_tool_trait_label_and_description() {
        let adapters = OpenApiToolAdapter::from_str(
            PETSTORE_JSON,
            OpenApiConfig::default(),
            &OperationFilter::All,
        )
        .unwrap();
        let list_pets = adapters.iter().find(|a| a.name() == "listPets").unwrap();
        assert_eq!(list_pets.label(), "List all pets");
        // description falls back to summary when no description field
        assert_eq!(list_pets.description(), "List all pets");
    }

    #[test]
    fn test_no_operations_returns_empty() {
        let filter = OperationFilter::ByOperationId(vec!["nonExistent".into()]);
        let result = OpenApiToolAdapter::from_str(PETSTORE_JSON, OpenApiConfig::default(), &filter);
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_no_base_url_error() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0.0" },
            "paths": {
                "/test": {
                    "get": {
                        "operationId": "test",
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"#;
        let result =
            OpenApiToolAdapter::from_str(spec, OpenApiConfig::default(), &OperationFilter::All);
        assert!(matches!(result.unwrap_err(), OpenApiError::NoBaseUrl));
    }

    #[test]
    fn test_base_url_from_config_overrides_spec() {
        let config = OpenApiConfig::default().with_base_url("https://custom.example.com");
        let adapters =
            OpenApiToolAdapter::from_str(PETSTORE_JSON, config, &OperationFilter::All).unwrap();
        assert_eq!(adapters[0].base_url, "https://custom.example.com");
    }

    #[test]
    fn test_base_url_trailing_slash_normalized() {
        let config = OpenApiConfig::default().with_base_url("https://custom.example.com/");
        let adapters =
            OpenApiToolAdapter::from_str(PETSTORE_JSON, config, &OperationFilter::All).unwrap();
        assert_eq!(adapters[0].base_url, "https://custom.example.com");
    }

    #[test]
    fn test_ref_resolution_parameters() {
        let spec = r##"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0.0" },
            "servers": [{ "url": "https://api.example.com" }],
            "paths": {
                "/items/{itemId}": {
                    "get": {
                        "operationId": "getItem",
                        "parameters": [
                            { "$ref": "#/components/parameters/ItemId" }
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            },
            "components": {
                "parameters": {
                    "ItemId": {
                        "name": "itemId",
                        "in": "path",
                        "required": true,
                        "schema": { "type": "string" }
                    }
                }
            }
        }"##;
        let adapters =
            OpenApiToolAdapter::from_str(spec, OpenApiConfig::default(), &OperationFilter::All)
                .unwrap();
        assert_eq!(adapters.len(), 1);
        let schema = adapters[0].parameters_schema();
        assert!(schema["properties"]["itemId"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("itemId")));
    }

    #[test]
    fn test_ref_resolution_schemas() {
        let spec = r##"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0.0" },
            "servers": [{ "url": "https://api.example.com" }],
            "paths": {
                "/items": {
                    "post": {
                        "operationId": "createItem",
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Item" }
                                }
                            }
                        },
                        "responses": { "201": { "description": "created" } }
                    }
                }
            },
            "components": {
                "schemas": {
                    "Item": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "price": { "type": "number" }
                        },
                        "required": ["name"]
                    }
                }
            }
        }"##;
        let adapters =
            OpenApiToolAdapter::from_str(spec, OpenApiConfig::default(), &OperationFilter::All)
                .unwrap();
        assert_eq!(adapters.len(), 1);
        let schema = adapters[0].parameters_schema();
        // Body schema should be present
        assert!(schema["properties"]["body"].is_object());
    }

    #[test]
    fn test_operations_without_id_are_skipped() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0.0" },
            "servers": [{ "url": "https://api.example.com" }],
            "paths": {
                "/test": {
                    "get": {
                        "responses": { "200": { "description": "ok" } }
                    }
                },
                "/other": {
                    "get": {
                        "operationId": "other",
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"#;
        let adapters =
            OpenApiToolAdapter::from_str(spec, OpenApiConfig::default(), &OperationFilter::All)
                .unwrap();
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].name(), "other");
    }

    #[test]
    fn test_percent_encode_path_segment() {
        assert_eq!(percent_encode_path_segment("hello"), "hello");
        assert_eq!(percent_encode_path_segment("hello world"), "hello%20world");
        assert_eq!(percent_encode_path_segment("foo/bar"), "foo%2Fbar");
        assert_eq!(percent_encode_path_segment("../admin"), "..%2Fadmin");
        assert_eq!(percent_encode_path_segment("a?b=c#d"), "a%3Fb%3Dc%23d");
    }

    #[test]
    fn test_parse_spec_json_error_message() {
        let result = parse_spec(r#"{ "not": "openapi" }"#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("JSON"), "Expected JSON error, got: {}", err);
    }

    #[test]
    fn test_parse_spec_yaml_error_message() {
        let result = parse_spec("not: valid: openapi: spec");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("YAML"), "Expected YAML error, got: {}", err);
    }

    #[test]
    fn test_broken_request_body_ref_errors() {
        let spec = r##"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0.0" },
            "servers": [{ "url": "https://api.example.com" }],
            "paths": {
                "/items": {
                    "post": {
                        "operationId": "createItem",
                        "requestBody": {
                            "$ref": "#/components/requestBodies/NonExistent"
                        },
                        "responses": { "201": { "description": "created" } }
                    }
                }
            }
        }"##;
        let result =
            OpenApiToolAdapter::from_str(spec, OpenApiConfig::default(), &OperationFilter::All);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("No components section") || err.contains("Request body not found"),
            "Expected request body resolution error, got: {}",
            err
        );
    }

    #[test]
    fn test_auth_debug_redacts_secrets() {
        let bearer = OpenApiAuth::Bearer("secret-token".into());
        let debug = format!("{:?}", bearer);
        assert!(!debug.contains("secret-token"));
        assert!(debug.contains("****"));

        let api_key = OpenApiAuth::ApiKey {
            header: "X-API-Key".into(),
            value: "secret-value".into(),
        };
        let debug = format!("{:?}", api_key);
        assert!(!debug.contains("secret-value"));
        assert!(debug.contains("X-API-Key"));
        assert!(debug.contains("****"));
    }
}
