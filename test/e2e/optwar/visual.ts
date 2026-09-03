import type { PpmImage } from "../engine-e2e.js";

const PLUGIN_PROMPT_TOP = { x: 0, y: 0, width: 240, height: 186 };
const PLUGIN_PROMPT_BODY = { x: 0, y: 186, width: 240, height: 103 };

function countMatchingPixels(
  image: PpmImage,
  rect: { x: number; y: number; width: number; height: number },
  predicate: (red: number, green: number, blue: number) => boolean,
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      if (predicate(...image.pixel(x, y))) count += 1;
    }
  }
  return count;
}

export function isPluginPrompt(image: PpmImage): boolean {
  const grayTopPixels = countMatchingPixels(
    image,
    PLUGIN_PROMPT_TOP,
    (red, green, blue) => red === 120 && green === 124 && blue === 120,
  );
  const lightBodyPixels = countMatchingPixels(
    image,
    PLUGIN_PROMPT_BODY,
    (red, green, blue) => red === 232 && green === 240 && blue === 248,
  );
  return grayTopPixels > 40_000
    && lightBodyPixels > 17_000
    && image.pixel(120, 300).toString() === "40,176,216"
    && image.pixel(10, 310).toString() === "248,252,248";
}
