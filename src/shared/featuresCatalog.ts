import catalog from "../../website/features-catalog.json";

export interface FeatureItem {
  name: string;
  detail?: string;
}

export interface FeatureSection {
  id: string;
  title: string;
  items: FeatureItem[];
}

export interface PlannedFeature {
  name: string;
  detail: string;
  tag?: "raycast" | "caduceus";
}

export const SHIPPED_SECTIONS = catalog.shipped as FeatureSection[];
export const PLANNED_FEATURES = catalog.planned as PlannedFeature[];

export function countShippedFeatures(): number {
  return SHIPPED_SECTIONS.reduce((n, s) => n + s.items.length, 0);
}
