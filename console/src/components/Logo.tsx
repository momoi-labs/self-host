import lockupDark from "../../../assets/brand/lockup-dark.svg";
import lockupLight from "../../../assets/brand/lockup-light.svg";
import symbolDark from "../../../assets/brand/symbol-dark.svg";
import symbolLight from "../../../assets/brand/symbol.svg";

export function BrandSymbol({ label = "" }: { label?: string }) {
  return (
    <span className="brand-symbol" aria-hidden={label ? undefined : "true"}>
      <img className="brand-on-light" src={symbolLight} alt={label} />
      <img className="brand-on-dark" src={symbolDark} alt={label} />
    </span>
  );
}

export function Lockup() {
  return (
    <span className="brand-lockup">
      <img className="brand-on-light" src={lockupLight} alt="self-host" />
      <img className="brand-on-dark" src={lockupDark} alt="self-host" />
    </span>
  );
}
