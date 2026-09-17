// Tiny i18n: English strings are the keys; `t()` looks them up in the active language's
// dictionary and falls back to the key itself. The language is a signal, so any component
// that calls t() during render re-renders on switch. Persisted in settings ("auto" follows
// navigator.language).

import { signal } from "@preact/signals";

export type Lang = "en" | "ru";
export type LangPref = Lang | "auto";

export const lang = signal<Lang>("en");

export function detectLang(): Lang {
  return (navigator.language || "").toLowerCase().startsWith("ru") ? "ru" : "en";
}

export function applyLang(pref: LangPref) {
  lang.value = pref === "auto" ? detectLang() : pref;
  document.documentElement.lang = lang.value;
}

export function t(s: string): string {
  return lang.value === "ru" ? (RU[s] ?? s) : s;
}

const RU: Record<string, string> = {
  // navigation / chrome
  "Calendar": "Календарь",
  "Board": "Доска",
  "Tasks": "Задачи",
  "Focus": "Фокус",
  "Command palette": "Командная палитра",
  "Undo": "Отменить",
  "Redo": "Повторить",
  "Toggle theme": "Сменить тему",
  "Settings": "Настройки",
  "Sign out": "Выйти",
  "unsynced changes": "несинхронизированные изменения",
  "Server unreachable — retrying…": "Сервер недоступен — повторяем попытку…",
  "panels": "панели",
  "palette": "палитра",
  "help": "справка",
  "new": "создать",
  "undo/redo": "отмена/повтор",
  "drag events to reschedule": "перетащите событие, чтобы перенести",
  "This event repeats — moving it moves the entire series.": "Событие повторяется — перенос сдвинет всю серию.",

  // quick add
  "Quick add task…": "Быстро добавить задачу…",
  "Quick add task…  #project @tomorrow !high": "Быстро добавить задачу…  #проект @завтра !высокий",
  "Add task": "Добавить задачу",
  "More options": "Больше полей",
  "Add to current project": "Добавлять в текущий проект",
  "Select a single project to file new tasks under it": "Выберите один проект, чтобы новые задачи попадали в него",
  "Reorder projects": "Изменить порядок проектов",
  "Give the task a title": "Укажите название задачи",
  "Task added — not in this view. Check “All”.": "Задача добавлена — её нет в этом виде. Смотрите «Все».",
  "Tags": "Метки",
  "comma-separated": "через запятую",

  // login
  "Create the admin account for this server.": "Создайте учётную запись администратора для этого сервера.",
  "New admin password": "Новый пароль администратора",
  "Password": "Пароль",
  "Confirm password": "Подтвердите пароль",
  "2FA code": "Код 2FA",
  "Create admin": "Создать администратора",
  "Sign in": "Войти",
  "password must be at least 8 characters": "пароль должен быть не короче 8 символов",
  "passwords do not match": "пароли не совпадают",
  "setup failed": "не удалось выполнить настройку",
  "login failed": "не удалось войти",

  // calendar
  "Today": "Сегодня",
  "month": "месяц",
  "week": "неделя",
  "day": "день",
  "Search events…": "Поиск событий…",
  "New event": "Новое событие",
  "Wk": "Нед",

  // tasks
  "All projects": "Все проекты",
  "No project": "Без проекта",
  "Clear filter": "Сбросить фильтр",
  "Search tasks…": "Поиск задач…",
  "Sort": "Сортировка",
  "open": "открытых",
  "Done": "Готово",
  "No tasks in this view.": "Нет задач в этом виде.",
  "due": "срок",

  // focus
  "idle": "ожидание",
  "paused": "пауза",
  "focus": "фокус",
  "break": "перерыв",
  "Start pomodoro": "Запустить помодоро",
  "Flowtime": "Флоутайм",
  "Pause": "Пауза",
  "Resume": "Продолжить",
  "Skip": "Пропустить",
  "Stop": "Стоп",
  "Next:": "Далее:",
  "Focus finished — time for a break": "Фокус завершён — время перерыва",
  "Break finished — back to focus": "Перерыв завершён — снова фокус",

  // bulk bar
  "selected": "выбрано",
  "Priority": "Приоритет",
  "Project": "Проект",
  "Delete": "Удалить",

  // help
  "Keyboard shortcuts": "Горячие клавиши",
  "Calendar / Board / Tasks / Focus": "Календарь / Доска / Задачи / Фокус",
  "Keyboard help": "Справка по клавишам",
  "New (event on Calendar, else task)": "Создать (событие в календаре, иначе задачу)",
  "Undo / Redo": "Отменить / Повторить",
  "Trash": "Корзина",
  "Close modal / clear selection": "Закрыть окно / снять выделение",
  "Drag events to reschedule; drag their edges to resize. Long-press a card to multi-select.":
    "Перетаскивайте события, чтобы переносить; тяните за края, чтобы менять длительность. Долгое нажатие на карточку — множественный выбор.",
  "Close": "Закрыть",

  // command palette
  "Type a command…": "Введите команду…",
  "no matching command": "нет подходящей команды",
  "no tasks selected": "задачи не выбраны",
  "Go to Calendar": "Перейти: календарь",
  "Go to Board": "Перейти: доска",
  "Go to Tasks": "Перейти: задачи",
  "Go to Focus": "Перейти: фокус",
  "New task": "Новая задача",
  "Mark selected done": "Отметить выбранные готовыми",
  "Delete selected": "Удалить выбранные",
  "Due: today": "Срок: сегодня",
  "Due: tomorrow": "Срок: завтра",
  "Due: end of week": "Срок: конец недели",
  "Due: next week": "Срок: следующая неделя",
  "Due: clear": "Срок: убрать",
  "Priority: high": "Приоритет: высокий",
  "Priority: medium": "Приоритет: средний",
  "Priority: low": "Приоритет: низкий",
  "Priority: none": "Приоритет: нет",
  "Open trash": "Открыть корзину",
  "Empty trash": "Очистить корзину",

  // settings
  "Theme": "Тема",
  "light": "светлая",
  "dark": "тёмная",
  "system": "системная",
  "Time format": "Формат времени",
  "24-hour": "24 часа",
  "12-hour": "12 часов",
  "Secondary timezone (calendar gutter)": "Дополнительный часовой пояс (календарь)",
  "None": "Нет",
  "Language": "Язык",
  "Auto": "Авто",
  "press a key…": "нажмите клавишу…",
  "Reset shortcuts": "Сбросить клавиши",
  "Notifications": "Уведомления",
  "Enable notifications": "Включить уведомления",
  "Notifications are blocked by the browser — allow them in site settings.":
    "Уведомления заблокированы браузером — разрешите их в настройках сайта.",
  "Notify for event/task reminders and pomodoro phases while the app is open.":
    "Напоминания о событиях/задачах и фазах помодоро, пока приложение открыто.",
  "Calendar panel": "Панель календаря",
  "Board panel": "Панель доски",
  "Tasks panel": "Панель задач",
  "Focus panel": "Панель фокуса",
  "New item": "Новый элемент",
  "Help": "Справка",
  "Users": "Пользователи",
  "No additional users yet.": "Дополнительных пользователей пока нет.",
  "Add user": "Добавить пользователя",
  "CalDAV accounts": "Аккаунты CalDAV",
  "No CalDAV accounts yet.": "Аккаунтов CalDAV пока нет.",
  "Discover calendars": "Найти календари",
  "Discovering…": "Поиск…",
  "No calendars found.": "Календари не найдены.",
  "Google Calendar": "Google Календарь",
  "Connect with Google": "Подключить Google",
  "Change OAuth client": "Изменить OAuth-клиент",
  "Save OAuth client": "Сохранить OAuth-клиент",

  // forms (task/event)
  "Title": "Название",
  "Notes": "Заметки",
  "Date": "Дата",
  "Low": "Низкий",
  "Medium": "Средний",
  "High": "Высокий",
  "Conference / video call": "Конференция / видеозвонок",
  "Join": "Присоединиться",
  "Description": "Описание",
  "Location": "Место",
  "Status": "Статус",
  "Due": "Срок",
  "Scheduled": "Запланировано",
  "Start": "Начало",
  "End": "Конец",
  "All day": "Весь день",
  "Create": "Создать",
  "Save": "Сохранить",
  "Cancel": "Отмена",
  "Edit task": "Изменить задачу",
  "Edit event": "Изменить событие",

  // trash
  "Restore": "Восстановить",
  "Purge": "Удалить навсегда",
  "Trash is empty.": "Корзина пуста.",
  "task": "задача",
  "proj": "проект",
  'Permanently delete "{name}"?': "Безвозвратно удалить «{name}»?",
  "Empty the trash? This cannot be undone.": "Очистить корзину? Это действие необратимо.",

  // login / auth errors
  "Email (leave empty for admin)": "Email (пусто — для администратора)",
  "invalid credentials": "неверные учётные данные",
  "unauthorized": "требуется вход",
  "too many attempts, locked for {s}s": "слишком много попыток — заблокировано на {s} с",

  // invite page
  "This invite link is invalid or has already been used. Ask your admin for a new one.":
    "Эта ссылка-приглашение недействительна или уже использована. Запросите новую у администратора.",
  "Welcome": "Добро пожаловать",
  "set a password to activate your account.": "задайте пароль, чтобы активировать учётную запись.",
  "New password": "Новый пароль",
  "Set password & sign in": "Задать пароль и войти",
  "invite failed": "не удалось принять приглашение",

  // change password
  "Change password": "Сменить пароль",
  "Current password": "Текущий пароль",
  "password changed": "пароль изменён",
  "password change failed": "не удалось сменить пароль",
  "current credentials are wrong": "текущие учётные данные неверны",
  "password too short (min 8)": "пароль слишком короткий (минимум 8 символов)",
  "Account": "Учётная запись",
  "sign out failed": "не удалось выйти",

  // admin: users + invites
  "user created": "пользователь создан",
  "create failed": "не удалось создать",
  "sync URL copied to clipboard": "ссылка синхронизации скопирована",
  "Sync URL for {id} (copy it now — the token is shown once):":
    "Ссылка синхронизации для {id} (скопируйте сейчас — токен показывается один раз):",
  "could not generate URL": "не удалось создать ссылку",
  "invite link copied to clipboard": "ссылка-приглашение скопирована",
  "Invite link for {id} (copy it now — it is shown once):":
    "Ссылка-приглашение для {id} (скопируйте сейчас — она показывается один раз):",
  "could not generate invite": "не удалось создать приглашение",
  'Delete user "{id}"? Their vault files are left on disk.':
    "Удалить пользователя «{id}»? Файлы его хранилища останутся на диске.",
  "delete failed": "не удалось удалить",
  "invite pending": "приглашение не принято",
  "Invite": "Пригласить",
  "Generate a one-time invite link": "Создать одноразовую ссылку-приглашение",
  "Copy an import/sync URL": "Скопировать ссылку импорта/синхронизации",
  "Sync URL": "Ссылка синхронизации",
  "Delete user": "Удалить пользователя",
  "id (a-z, 0-9, - _)": "id (a-z, 0-9, - _)",
  "display name (optional)": "отображаемое имя (необязательно)",
  "email (for invites, optional)": "email (для приглашений, необязательно)",

  // admin: Google OAuth
  "Google connected — calendars added (sync to pull them in)":
    "Google подключён — календари добавлены (запустите синхронизацию)",
  "Google OAuth client saved": "OAuth-клиент Google сохранён",
  "save failed": "не удалось сохранить",
  "could not start Google connect": "не удалось начать подключение Google",
  "Set web.public_origin to enable the Google connect flow.":
    "Укажите web.public_origin, чтобы включить подключение Google.",
  "Create an OAuth client (Web application) in Google Cloud, enable the Calendar API, and add this redirect URI:":
    "Создайте OAuth-клиент (Web application) в Google Cloud, включите Calendar API и добавьте этот redirect URI:",
  "<set public_origin first>": "<сначала задайте public_origin>",
  "client id": "client id",
  "client secret": "client secret",
  "account name": "имя аккаунта",

  // admin: CalDAV
  "discovery failed": "не удалось найти календари",
  "CalDAV account saved — sync with `mgmt sync` or the daemon":
    "Аккаунт CalDAV сохранён — синхронизируйте через `mgmt sync` или демон",
  'Remove CalDAV account "{name}" and its collections?': "Удалить аккаунт CalDAV «{name}» и его коллекции?",
  "remove failed": "не удалось удалить",
  "cal": "кал.",
  "Remove account": "Удалить аккаунт",
  "server URL": "URL сервера",
  "username": "имя пользователя",
  "token": "токен",
  "app password": "пароль приложения",
  "Yandex: use your login as the username and an app password for “Calendar CalDAV” (Yandex ID → Security → App passwords), not your main password.":
    "Яндекс: используйте свой логин и пароль приложения для «Календарь CalDAV» (Яндекс ID → Безопасность → Пароли приложений), а не основной пароль.",
  "events": "события",
  "tasks": "задачи",
  "account name (optional)": "имя аккаунта (необязательно)",
  "Save {n} calendar(s)": "Сохранить календарей: {n}",
  // admin: calendar sync (provider-first)
  "Calendar sync": "Синхронизация календаря",
  "No calendar accounts yet.": "Пока нет аккаунтов календаря.",
  "Add an account": "Добавить аккаунт",
  "Custom": "Другой",
  "Yandex": "Яндекс",
  "Fastmail": "Fastmail",
  "iCloud": "iCloud",
  "Radicale / self-hosted": "Radicale / свой сервер",
  "Server URL": "URL сервера",
  "Sign-in method": "Способ входа",
  "Username & password": "Логин и пароль",
  "Bearer token": "Bearer-токен",
  "Username": "Имя пользователя",
  "username / email": "логин / e-mail",
  "Token": "Токен",
  "App password": "Пароль приложения",
  "App-specific password": "Пароль для приложения",
  "Google uses a one-click sign-in — no password needed.": "Google использует вход в один клик — пароль не нужен.",
  "Yandex: username is your login; the password is an app password for “Calendar CalDAV” (Yandex ID → Security → App passwords), not your account password.":
    "Яндекс: имя пользователя — ваш логин; пароль — это пароль приложения для «Календарь CalDAV» (Яндекс ID → Безопасность → Пароли приложений), а не пароль от аккаунта.",
  "Fastmail: create an app password under Settings → Privacy & Security → App passwords.":
    "Fastmail: создайте пароль приложения в Settings → Privacy & Security → App passwords.",
  "iCloud: generate an app-specific password at appleid.apple.com → Sign-In and Security.":
    "iCloud: сгенерируйте пароль для приложения на appleid.apple.com → Вход и безопасность.",
  "Google sync isn't available on this server.": "Синхронизация с Google недоступна на этом сервере.",
  "Account name": "Имя аккаунта",

  // event/task forms
  "End must be after start": "Конец должен быть позже начала",
  "Reminders": "Напоминания",
  "Reminders must be offsets like 15m, 1h, 1d": "Напоминания — интервалы вида 15m, 1h, 1d",
  "e.g. 15m, 1h, 1d — empty for none": "напр. 15m, 1h, 1d — пусто, если не нужно",

  // recurrence editor
  "Repeats": "Повторение",
  "Does not repeat": "Не повторяется",
  "Daily": "Ежедневно",
  "Weekly": "Еженедельно",
  "Monthly": "Ежемесячно",
  "Yearly": "Ежегодно",
  "every": "каждые",
  "ends": "заканчивается",
  "never": "никогда",
  "after": "после",
  "on": "в дату",
  "Mon": "Пн",
  "Tue": "Вт",
  "Wed": "Ср",
  "Thu": "Чт",
  "Fri": "Пт",
  "Sat": "Сб",
  "Sun": "Вс",
  "days": "дн.",
  "weeks": "нед.",
  "months": "мес.",
  "year": "год",
  "years": "лет",

  // project picker
  "Assign project": "Назначить проект",
  "Search projects…": "Поиск проектов…",
  "(clear project)": "(убрать проект)",

  // local calendars (Settings → Calendars)
  "Calendars": "Календари",
  "Rename": "Переименовать",
  "Upload": "Загрузить",
  "Download": "Скачать",
  "Add": "Добавить",
  "Imported": "Импортировано",
  "New calendar name": "Название календаря",
  "Move its events to 'default' and delete this calendar?": "Перенести события в «default» и удалить этот календарь?",

  // misc
  "OK": "ОК",
  "request failed": "запрос не выполнен",
  "Delete {n} selected task(s)?": "Удалить выбранные задачи ({n})?",
  "Clear selection": "Снять выделение",
  "Previous": "Назад",
  "Next": "Вперёд",
};
