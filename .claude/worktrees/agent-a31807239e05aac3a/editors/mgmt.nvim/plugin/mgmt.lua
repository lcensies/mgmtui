-- Entry point auto-sourced by any plugin manager. Guarded so that a machine without the `mgmt`
-- binary installed loads this with zero side effects and zero errors.

if vim.g.loaded_mgmt_nvim then
  return
end
vim.g.loaded_mgmt_nvim = true

if vim.fn.executable("mgmt") ~= 1 then
  return
end

require("mgmt").setup()
