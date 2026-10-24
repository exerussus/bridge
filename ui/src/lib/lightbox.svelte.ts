// Картинка из чата во весь экран: сохранить, а чужой стикер — забрать в свои.
export const LB = $state<{ cur: { src: string; dl: string; name: string; sticker: string } | null }>({ cur: null });
export function openLightbox(x: { src: string; dl: string; name: string; sticker: string }) { LB.cur = x; }
export function closeLightbox() { LB.cur = null; }
