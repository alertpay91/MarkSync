# MarkSync

Program za Windows koji poredi slike, snimke i druge fajlove između računara i eksternog diska, kopira samo ono što nedostaje i pomaže da se sve sredi.

## Šta radi

- **Sinhronizacija**: levo štikliraš foldere na računaru, desno biraš eksterni disk. MarkSync poredi fajlove **po sadržaju**, pa ime i folder nisu bitni: ista slika pod drugim imenom ili u drugom folderu (npr. posle filtera) prepoznaje se kao „Već je na disku“. Svaki fajl dobije oznaku:
  - **Novo**: nema ga na disku.
  - **Novija verzija**: isto ime i mesto, ali je fajl na računaru noviji. Podrazumevano se čuvaju oba, a „Zameni stari“ biraš sam.
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

  Datum se čita iz EXIF-a, iz MP4/MOV snimka, iz imena (`IMG_20240715…`, `PXL_20250905…`, `IMG-20240101-WA0001`) ili, ako ništa od toga ne postoji, iz datuma izmene fajla. Premeštanje može da se **poništi** jednim klikom, i posle nestanka struje.

  Dodatne opcije (sve su podrazumevano uključene):
  - **Albumi**: folder sa smislenim imenom ostaje kao podfolder (`2024\07 - Jul\Rođendan Mila`). „Nova fascikla“, „Camera“, „DCIM“, nazivi sa datumima i slično se preskaču.
  - **„_Proveri datum“**: slike i snimci bez pravog datuma snimanja (npr. skinuti sa Facebooka) idu posebno, da ne završe tiho u pogrešnoj godini.
  - **Duplikati**: ista slika se premešta samo jednom, a višak ide u Korpu.
- Posle kopiranja u sređene foldere, jedan klik **sredi i fajlove koji su već bili na disku** po istom rasporedu.
- **Duplikati**: pronalazi iste fajlove i višak šalje u Korpu ili u folder `_MarkSync duplikati`.
- **Istorija**: svako kopiranje, premeštanje i uklanjanje zapisano je u `%APPDATA%\MarkSync\Istorija` (Podešavanja → Istorija).
- Jezici: srpski (latinica i ćirilica), engleski, nemački i ruski. Jezik se bira pri instalaciji, a kasnije u Podešavanjima.

## Bezbednost

- Ništa se ne kopira ni briše dok ne vidiš spisak i ne potvrdiš.
- Fajlovi se porede po celom sadržaju (BLAKE3), a ne po imenu ili datumu.
- **Svaka kopija se proverava**: kopira se u privremeni fajl, zatim se zapisani fajl pročita ponovo i uporedi sa originalom. Tek tada dobija pravo ime. Postojeći fajl se nikad ne pregazi.
- Pre uklanjanja duplikata program ponovo proverava da zadržana kopija postoji i da je potpuno ista.
- Stara verzija koja se zamenjuje ide u Korpu. Ako disk nema Korpu, preimenuje se u „(stara verzija DATUM)“.
- Ako telefon prestane da odgovara, poređenje se prekida, umesto da prikaže pogrešan spisak.

## Telefon (Android, iPhone)

Poveži telefon USB kablom, otključaj ga, izaberi „Prenos fajlova“ i klikni **📱 Telefon**. Prečica „Slike i snimci sa kamere“ otvara `DCIM\Camera`. Telefon se otvara samo za čitanje: na njemu se ništa ne menja.

## Pravljenje

```powershell
.\build.ps1        # dist\MarkSync-Setup-<ver>.exe i dist\MarkSync-<ver>-portable.exe
```

Potrebni su Rust (stable), Python 3 (za ćirilični prevod installera) i Inno Setup 6.

Testovi rade nad realnim primerom:

```powershell
python tools\make_testdata.py C:\Temp\mstest
$env:MARKSYNC_TESTDATA="C:\Temp\mstest"; cargo test --release -- --test-threads=1 --skip phone
```

Testovi sa pravim telefonom (samo čitanje): `MARKSYNC_PHONE=1`, a za poređenje i `MARKSYNC_PHONE_TARGET=<folder>`.

## Podrška

Ako ti je MarkSync pomogao, možeš da me častiš kafom preko PayPal-a: alertpay91@gmail.com (dugmad su i u programu, u Podešavanjima).

Podaci o mestima: GeoNames (CC BY 4.0), preko biblioteke `reverse_geocoder`.
