import { createContext } from 'react';

import { THEMES, type Theme } from './tokens';

/**
 * The active resolved theme. Defaults to light so a consumer rendered outside a
 * [[ThemeProvider]] (e.g. a unit test) still gets a valid palette. The provider
 * overrides this with the OS-derived mode.
 */
export const ThemeContext = createContext<Theme>(THEMES.light);
