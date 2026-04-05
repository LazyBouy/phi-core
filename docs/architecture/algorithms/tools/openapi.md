<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `OpenApiToolAdapter::execute` *(src/openapi/)*

**Purpose:** Execute a single OpenAPI operation as an HTTP request.

```
FUNCTION OpenApiToolAdapter::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  // Normalize params: null → {}; non-object → error
  IF params is null THEN params ← {} END IF
  IF params is NOT object THEN
    RETURN Ok(ToolResult { content: [Text("Error: params must be an object")] })
  END IF

  // ── Step 1: Substitute path parameters ────────────────────────────────────
  url_path ← self.info.path  // e.g. "/users/{userId}/posts/{postId}"
  FOR EACH param_name IN self.info.path_params
    value ← params[param_name]
    IF value is missing THEN
      RETURN Ok(ToolResult { content: [Text("Error: missing required path param '{param_name}'")] })
    END IF
    encoded ← percent_encode_rfc3986(value.to_string())
    url_path ← replace(url_path, "{" + param_name + "}", encoded)
  END FOR

  // ── Step 2: Build base URL ─────────────────────────────────────────────────
  url ← self.base_url + url_path

  // ── Step 3: Build HTTP request ────────────────────────────────────────────
  method ← parse_http_method(self.info.method)  // GET, POST, PUT, etc.
  request ← self.client.request(method, url)

  // Query parameters
  FOR EACH param_name IN self.info.query_params
    IF params[param_name] defined THEN
      request ← request.query(param_name, params[param_name].to_string())
    END IF
  END FOR

  // Header parameters
  FOR EACH param_name IN self.info.header_params
    IF params[param_name] defined THEN
      request ← request.header(param_name, params[param_name].to_string())
    END IF
  END FOR

  // Authentication
  MATCH self.config.auth
    CASE None           → (no-op)
    CASE Bearer(token)  → request ← request.bearer_auth(token)
    CASE ApiKey{header,value} → request ← request.header(header, value)
  END MATCH

  // Custom headers
  FOR EACH (key, value) IN self.config.custom_headers
    request ← request.header(key, value)
  END FOR

  // Request body (application/json only)
  IF self.info.has_body THEN
    body ← params["body"] OR params["_request_body"]
    IF body defined THEN
      request ← request.json(body)
