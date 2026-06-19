-- mgmt.nvim — frontmatter completion for mgmt task markdown.
--
-- `setup()` registers an nvim-cmp source named "mgmt". It is safe to call unconditionally: if
-- the `mgmt` binary or nvim-cmp is missing, it quietly does nothing.

local M = {}

--- Register the cmp source. Idempotent and failure-tolerant.
function M.setup()
  if vim.fn.executable("mgmt") ~= 1 then
    return
  end
  local ok, cmp = pcall(require, "cmp")
  if not ok then
    return
  end
  cmp.register_source("mgmt", require("mgmt.source").new())

  -- Refresh the cached schema when a task file is written (statuses/projects may have changed).
  local group = vim.api.nvim_create_augroup("MgmtNvim", { clear = true })
  vim.api.nvim_create_autocmd("BufWritePost", {
    group = group,
    pattern = "*.md",
    callback = function()
      require("mgmt.meta").invalidate()
    end,
  })
end

return M
