import { ref, watch } from 'vue'

export type ThemeMode = 'light' | 'dark' | 'system'

type ThemeValue = 10 | 20 | 30

const STORAGE_KEY = 'DesktopConfig-1000'

const themeMode = ref<ThemeMode>('system')
let isInitialized = false
let lastThemeValue: number | null = null
// polling interval id removed to avoid unused variable lint error

function themeValueToMode(value: number): ThemeMode {
  switch (value) {
    case 10:
      return 'light'
    case 20:
      return 'dark'
    case 30:
      return 'system'
    default:
      return 'system'
  }
}

function modeToThemeValue(mode: ThemeMode): ThemeValue {
  switch (mode) {
    case 'light':
      return 10
    case 'dark':
      return 20
    case 'system':
      return 30
  }
}

function getSystemTheme(): 'light' | 'dark' {
  if (typeof window !== 'undefined' && window.matchMedia) {
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
  }
  return 'light'
}

function applyTheme(mode: ThemeMode) {
  const effectiveTheme = mode === 'system' ? getSystemTheme() : mode
  
  if (effectiveTheme === 'dark') {
    document.documentElement.classList.add('dark')
    document.body.setAttribute('theme-mode', 'dark')
  } else {
    document.documentElement.classList.remove('dark')
    document.body.removeAttribute('theme-mode')
  }
}

function loadTheme(): ThemeMode {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved) {
      const config = JSON.parse(saved)
      if (config?.userPreference?.theme !== undefined) {
        lastThemeValue = config.userPreference.theme
        return themeValueToMode(config.userPreference.theme)
      }
    }
  } catch (e) {
    console.warn('Failed to load theme from localStorage:', e)
  }
  return 'system'
}

function saveTheme(mode: ThemeMode) {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    let config: Record<string, unknown> = {}
    
    if (saved) {
      try {
        config = JSON.parse(saved)
      } catch {
        config = {}
      }
    }
    
    if (!config.userPreference) {
      config.userPreference = {}
    }
    
    const themeValue = modeToThemeValue(mode)
    lastThemeValue = themeValue
    ;(config.userPreference as Record<string, unknown>).theme = themeValue
    
    localStorage.setItem(STORAGE_KEY, JSON.stringify(config))
  } catch (e) {
    console.warn('Failed to save theme to localStorage:', e)
  }
}

function checkLocalStorageChanges() {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved) {
      const config = JSON.parse(saved)
      if (config?.userPreference?.theme !== undefined) {
        const newValue = config.userPreference.theme
        if (newValue !== lastThemeValue) {
          lastThemeValue = newValue
          themeMode.value = themeValueToMode(newValue)
        }
      }
    }
  } catch (e) {
    console.warn('Failed to check localStorage changes:', e)
  }
}

function handleStorageChange(e: StorageEvent) {
  if (e.key === STORAGE_KEY && e.newValue) {
    try {
      const config = JSON.parse(e.newValue)
      if (config?.userPreference?.theme !== undefined) {
        const newMode = themeValueToMode(config.userPreference.theme)
        lastThemeValue = config.userPreference.theme
        themeMode.value = newMode
      }
    } catch {
      console.warn('Failed to parse theme from storage event')
    }
  }
}

function handleSystemThemeChange() {
  if (themeMode.value === 'system') {
    applyTheme('system')
  }
}

function initializeTheme() {
  if (isInitialized) return
  
  themeMode.value = loadTheme()
  applyTheme(themeMode.value)

  window.addEventListener('storage', handleStorageChange)

  if (window.matchMedia) {
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
    mediaQuery.addEventListener('change', handleSystemThemeChange)
  }

  window.setInterval(checkLocalStorageChanges, 500)

  watch(themeMode, (newMode) => {
    applyTheme(newMode)
    saveTheme(newMode)
  })

  isInitialized = true
}

export function useTheme() {
  if (!isInitialized) {
    initializeTheme()
  }

  const setTheme = (mode: ThemeMode) => {
    themeMode.value = mode
  }

  const toggleTheme = () => {
    const currentEffective = themeMode.value === 'system' ? getSystemTheme() : themeMode.value
    setTheme(currentEffective === 'dark' ? 'light' : 'dark')
  }

  return {
    themeMode,
    setTheme,
    toggleTheme,
    getSystemTheme
  }
}
