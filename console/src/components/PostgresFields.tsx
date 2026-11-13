import { FormField, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@momoi-labs/kiso-react";

import type { CustomImage } from "../lib/types.js";

/** The value of the official image in the Image field: no custom image. */
const OFFICIAL = "official";

export function PostgresFields({ major, onChange, image, onImage, images }: {
  major: number;
  onChange: (major: number) => void;
  /** The id of the chosen custom image, or null for the official one. */
  image: string | null;
  onImage: (image: string | null) => void;
  /** The custom images that are built. */
  images: CustomImage[];
}) {
  return <>
    <Select value={String(major)} onValueChange={(value) => onChange(Number(value))}>
      <FormField id="postgres-major" label="PostgreSQL version" hint="The major version stays fixed."><SelectTrigger><SelectValue /></SelectTrigger></FormField>
      <SelectContent>{[18, 17, 16].map((version) => <SelectItem key={version} value={String(version)}>PostgreSQL {version}</SelectItem>)}</SelectContent>
    </Select>
    <Select value={image ?? OFFICIAL} onValueChange={(value) => onImage(value === OFFICIAL ? null : value)}>
      <FormField id="postgres-image" label="Image"
        hint="A custom image adds extensions. Build one from the PostgreSQL template under Custom images. It must run the version above, and needs pgBackRest to be backed up.">
        <SelectTrigger><SelectValue /></SelectTrigger>
      </FormField>
      <SelectContent>
        <SelectItem value={OFFICIAL}>Official PostgreSQL image</SelectItem>
        {images.map((item) => <SelectItem key={item.id} value={item.id}>{item.name}</SelectItem>)}
      </SelectContent>
    </Select>
  </>;
}
