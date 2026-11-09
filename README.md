# MarkSync

Program za Windows koji poredi slike, snimke i druge fajlove između računara i eksternog diska, kopira samo ono što nedostaje i pomaže da se sve sredi.

## Šta radi

- **Sinhronizacija**: levo štikliraš foldere na računaru, desno biraš eksterni disk. MarkSync poredi fajlove **po sadržaju**, pa ime i folder nisu bitni: ista slika pod drugim imenom ili u drugom folderu (npr. posle filtera) prepoznaje se kao „Već je na disku“. Svaki fajl dobije oznaku:
  - **Novo**: nema ga na disku.
  - **Novija verzija**: isto ime i mesto, ali je fajl na računaru noviji. Ponuđeno je „Zameni stari“ ili „Sačuvaj oba“.
  - **Isto ime**: na disku postoji drugačiji fajl sa istim imenom. Podrazumevano se čuvaju oba (`ime (2).jpg`).
  - **Već je na disku** i **Duplikat u izvoru**: preskaču se.
- Nove fajlove na disku može da složi u **iste foldere**, **sve u jedan folder** ili **automatski sređeno**.
- **Sređivanje** foldera ili celog diska, po izboru:
  - godina / mesec
  - godina / mesec / dan
  - samo godina
  - mesto (država / grad, iz GPS-a u slici, bez interneta)
  - godina pa mesto
  - abeceda
  - vrsta fajla
  - telefon / aparat

  Datum se čita iz EXIF-a, iz MP4/MOV snimka, iz imena (`IMG_20240715…`, `IMG-20240101-WA0001`) ili, ako ništa od toga ne postoji, iz datuma izmene fajla. Premeštanje može da se **poništi** jednim klikom.
- **Duplikati**: pronalazi iste fajlove i višak šalje u Korpu ili u folder `_MarkSync duplikati`.
- Jezici: srpski (latinica i ćirilica), engleski, nemački i ruski. Jezik se bira pri instalaciji, a kasnije u Podešavanjima.

## Bezbednost

- Ništa se ne kopira ni briše dok ne vidiš spisak i ne potvrdiš.
- Kopira se preko privremenog fajla, uz proveru veličine, i čuva se originalni datum.
- Stara verzija koja se zamenjuje ide u Korpu. Ako disk nema Korpu, preimenuje se u „(stara verzija DATUM)“.

## Telefon (Android)

Windows ne daje programima pristup telefonu kao običnom disku (MTP). Zato telefon poveži USB kablom, izaberi „Prenos fajlova“, prekopiraj folder `DCIM` na računar i onda pokreni MarkSync.

## Pravljenje

```powershell
.\build.ps1        # dist\MarkSync-Setup-<ver>.exe i dist\MarkSync-<ver>-portable.exe
```

Potrebni su Rust (stable), Python 3 (za ćirilični prevod installera) i Inno Setup 6.

Testovi rade nad realnim primerom:

```powershell
python tools\make_testdata.py C:\Temp\mstest
$env:MARKSYNC_TESTDATA="C:\Temp\mstest"; cargo test --release -- --test-threads=1
```

## Podrška

Ako ti je MarkSync pomogao, možeš da me častiš kafom preko PayPal-a: alertpay91@gmail.com (dugmad su i u programu, u Podešavanjima).

Podaci o mestima: GeoNames (CC BY 4.0), preko biblioteke `reverse_geocoder`.
