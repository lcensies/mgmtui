-- Fetch the task-metadata schema from the `mgmt` CLI and cache it for the session.
--
-- Everything here degrades to an empty schema if the binary is missing or errors, so callers
-- never have to handle failures — they just get no completions.

local M = {}

local cache = nil

--- Return the cached schema, fetching it once via `mgmt meta --json`.
--- @return table schema  (empty table {} when mgmt is unavailable or fails)
function M.get()
  if cache ~= nil then
    return cache
  end
  cache = {}

  if vim.fn.executable("mgmt") ~= 1 then
    return cache
  end

  local ok, out = pcall(vim.fn.system, { "mgmt", "meta", "--json" })
  if not ok or vim.v.shell_error ~= 0 or type(out) ~= "string" or out == "" then
    return cache
  end

  local decoded_ok, decoded = pcall(vim.json.decode, out)
  if decoded_ok and type(decoded) == "table" then
    cache = decoded
  end
  return cache
end

--- Drop the cached schema so the next `get()` re-queries the CLI (e.g. after editing config).
function M.invalidate()
  cache = nil
end

return M
