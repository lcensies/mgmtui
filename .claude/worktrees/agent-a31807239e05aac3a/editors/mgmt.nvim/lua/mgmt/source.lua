-- An nvim-cmp completion source for mgmt task markdown frontmatter.
--
-- It completes frontmatter *keys* (status:, priority:, project:, …) and, once you're past a
-- `key:`, the *values* valid for that key (configured status ids, known projects, etc.). It is
-- only active inside the leading `---`…`---` block of a buffer that looks like an mgmt task, and
-- it goes completely silent when the `mgmt` binary is absent.

local meta = require("mgmt.meta")

local source = {}

function source.new()
  return setmetatable({}, { __index = source })
end

-- Which frontmatter values each key offers (resolved against the live schema).
local VALUE_KEYS = {
  status = "statuses",
  priority = "priorities",
  project = "projects",
  area = "areas",
  tags = "tags",
  reminders = "reminders",
}

--- True when the cursor is inside the leading `---`…`---` frontmatter block.
local function in_frontmatter()
  local lines = vim.api.nvim_buf_get_lines(0, 0, 1, false)
  if lines[1] ~= "---" then
    return false
  end
  local row = vim.api.nvim_win_get_cursor(0)[1] -- 1-indexed
  -- Find the closing fence; the cursor must be strictly before it.
  local all = vim.api.nvim_buf_get_lines(0, 1, row, false)
  for _, l in ipairs(all) do
    if l == "---" then
      return false
    end
  end
  return true
end

--- Heuristic: is this buffer an mgmt task file? Either it lives under the vault's tasks dir, or
--- its frontmatter carries a `uid:` line (the signature of a task document).
local function is_task_buffer(schema)
  if vim.bo.filetype ~= "markdown" then
    return false
  end
  local name = vim.api.nvim_buf_get_name(0)
  local tasks_dir = schema.tasks_dir
  if tasks_dir and tasks_dir ~= "" and name:sub(1, #tasks_dir) == tasks_dir then
    return true
  end
  for i, l in ipairs(vim.api.nvim_buf_get_lines(0, 0, 15, false)) do
    if l:match("^uid:%s*%S") then
      return true
    end
    if l == "---" and i > 1 then
      break
    end
  end
  return false
end

function source:is_available()
  local schema = meta.get()
  if not schema.fields then
    return false
  end
  return is_task_buffer(schema) and in_frontmatter()
end

function source:get_trigger_characters()
  return { ":", " " }
end

function source:get_keyword_pattern()
  return [[\k\+]]
end

function source:complete(_, callback)
  local schema = meta.get()
  local line = vim.api.nvim_get_current_line()
  local col = vim.api.nvim_win_get_cursor(0)[2]
  local before = line:sub(1, col)

  local items = {}
  local key = before:match("^(%w[%w_%-]*):")
  if key then
    -- Completing a value for `key:`.
    local field = VALUE_KEYS[key]
    local values = field and schema[field] or nil
    if type(values) == "table" then
      for _, v in ipairs(values) do
        table.insert(items, { label = v, kind = vim.lsp.protocol.CompletionItemKind.Value })
      end
    end
  else
    -- Completing a frontmatter key.
    for _, f in ipairs(schema.fields or {}) do
      table.insert(items, {
        label = f,
        insertText = f .. ": ",
        kind = vim.lsp.protocol.CompletionItemKind.Field,
      })
    end
  end

  callback({ items = items, isIncomplete = false })
end

return source
