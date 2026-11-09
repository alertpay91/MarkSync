"""Generates installer/SerbianCyrillic.isl from SerbianLatin.isl (and adds the UTF-8 BOM Inno Setup needs)."""
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
INST = os.path.join(HERE, "..", "installer")

KEEP = ["MarkSync", "GB", "MB"]
DIGRAPHS = {"Lj": "Љ", "LJ": "Љ", "lj": "љ", "Nj": "Њ", "NJ": "Њ", "nj": "њ", "Dž": "Џ", "DŽ": "Џ", "dž": "џ"}
SINGLE = dict(zip("abcčćdđefghijklmnoprsštuvzžABCČĆDĐEFGHIJKLMNOPRSŠTUVZŽ",
                  "абцчћдђефгхијклмнопрсштувзжАБЦЧЋДЂЕФГХИЈКЛМНОПРСШТУВЗЖ"))


def translit(s: str) -> str:
    out, i = [], 0
    while i < len(s):
        if s[i] == "[":  # [name], [name/ver], [gb] are Inno placeholders
            j = s.find("]", i)
            if j > 0:
                out.append(s[i:j + 1]); i = j + 1; continue
        if s[i] == "%" and i + 1 < len(s):  # %1, %n
            out.append(s[i:i + 2]); i += 2; continue
        if i == 0 or not s[i - 1].isalnum():
            for w in KEEP:
                if s.startswith(w, i) and (i + len(w) == len(s) or not s[i + len(w)].isalnum()):
                    out.append(w); i += len(w); break
            else:
                w = None
            if w:
                continue
        if s[i:i + 2] in DIGRAPHS:
            out.append(DIGRAPHS[s[i:i + 2]]); i += 2; continue
        out.append(SINGLE.get(s[i], s[i])); i += 1
    return "".join(out)


def main():
    src = open(os.path.join(INST, "SerbianLatin.isl"), encoding="utf-8-sig").read()
    lines = []
    for line in src.splitlines():
        if line.startswith("LanguageName="):
            lines.append("LanguageName=Српски (ћирилица)")
        elif line.startswith("LanguageID="):
            lines.append("LanguageID=$281A")
        elif line.startswith(";") or line.startswith("[") or "=" not in line or line.startswith("LanguageCodePage"):
            lines.append(line)
        else:
            k, v = line.split("=", 1)
            lines.append(f"{k}={translit(v)}")
    text = "\n".join(lines) + "\n"
    open(os.path.join(INST, "SerbianCyrillic.isl"), "w", encoding="utf-8-sig", newline="\r\n").write(text)
    # make sure the Latin file has a BOM too
    open(os.path.join(INST, "SerbianLatin.isl"), "w", encoding="utf-8-sig", newline="\r\n").write(src.replace("\r\n", "\n"))
    print("ok")


if __name__ == "__main__":
    main()
