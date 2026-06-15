import { create } from 'zustand'
import type { SendMailDto } from './api'

// Annulation d'envoi façon Gmail : l'envoi réel est différé de quelques secondes ;
// pendant ce délai un toast « Annuler » permet de revenir à la rédaction.
interface UndoSendState {
  payload: SendMailDto | null
  _timer:  ReturnType<typeof setTimeout> | null
  /** Programme l'envoi dans `delayMs` ; affiche le toast. `onFire` effectue l'envoi réel. */
  schedule: (payload: SendMailDto, onFire: () => void, delayMs?: number) => void
  /** Annule l'envoi en attente et renvoie le brouillon pour ré-ouvrir la rédaction. */
  cancel: () => SendMailDto | null
}

export const useUndoSendStore = create<UndoSendState>((set, get) => ({
  payload: null,
  _timer:  null,
  schedule: (payload, onFire, delayMs = 5000) => {
    const prev = get()._timer
    if (prev) clearTimeout(prev)
    const timer = setTimeout(() => { onFire(); set({ payload: null, _timer: null }) }, delayMs)
    set({ payload, _timer: timer })
  },
  cancel: () => {
    const { _timer, payload } = get()
    if (_timer) clearTimeout(_timer)
    set({ payload: null, _timer: null })
    return payload
  },
}))
