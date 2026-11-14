#!/usr/bin/env python3
"""Everything the screencast shows that is written in a language: the demo file, what the user
types, what the scripted model answers, and the title cards. One entry per language.

    content.py <lang>      prints that language's entry as JSON (record.js reads it)

scenes.py and make_csv.py import it. The figures are the same in every language; they are what
the Python step prints for the demo file (see make_csv.py).
"""
import json
import re
import sys

LANGS = ("en", "de", "fr", "es")
NB = " "

# Category totals, share of the total and monthly average, in the order of `cats`.
TABLE = [(14500, 50.1, 1450), (5928, 20.5, 593), (2695, 9.3, 270), (1834, 6.3, 183),
         (1445, 5.0, 145), (1428, 4.9, 143), (1129, 3.9, 113)]


def eur(lang, n):
    """An amount of euros the way the language writes it."""
    if lang == "en":
        return f"€{n:,}"
    if lang == "de":
        return f"{n:,}".replace(",", ".") + NB + "€"
    if lang == "fr":
        return f"{n:,}".replace(",", NB) + NB + "€"
    # Latin American Spanish (es-419), as the chat formats numbers there: 28,959 and 50.1
    return f"{n:,}" + NB + "€"


def pct(lang, x):
    if lang == "en":
        return f"{x}%"
    return (f"{x}" if lang == "es" else f"{x}".replace(".", ",")) + NB + "%"


def french(text):
    """French typography: no-break spaces before : and €, and inside numbers."""
    text = re.sub(r" ([:€])", NB + r"\1", text)
    return re.sub(r"(\d) (\d{3})", r"\1" + NB + r"\2", text)


C = {
    "en": {
        "locale": "en-US", "hover_locale": "de-DE",
        "csv": "household-spending-2026.csv",
        "cols": ["date", "category", "description", "amount_eur"],
        "cats": ["Rent", "Groceries", "Dining out", "Transport", "Energy", "Other", "Subscriptions"],
        "descs": {},
        "months": ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct"],
        "py": {"comment": "Total the spending by month and category", "a_month": "a month", "total": "Total",
               "over": "over {n} months", "month": "month", "sum": "total"},
        "prompt": "Here is our household spending for this year. Work out where the money goes each month, "
                  "chart it, and write a one-page PDF summary with three ways to save.",
        "prompt_delete": "Thanks. Now delete the original CSV from the workspace.",
        "prompt_remember": "Remember that we want to keep our spending under 2,800 euros a month.",
        "plan": ["Read the spending file and check its columns", "Total the spending by month and category",
                 "Draw a chart of spending by month", "Find three realistic ways to save",
                 "Write the one-page PDF summary"],
        "say_read": "I'll read the file and total it by month and category.",
        "say_chart": "Rent is half of the spending. Dining out stands out in summer. Here is the chart.",
        "say_pdf": "Now the one-page summary.",
        "chart_title": "Household spending by month, 2026", "chart_file": "spending-by-month.png",
        "pdf_file": "spending-summary.pdf", "pdf_title": "Household spending 2026",
        "report": {
            "title": "Household spending, January to October 2026",
            "total": "**Total: {total} over ten months, about {avg} a month.**",
            "caption": "Spending by month and category",
            "head": ["Category", "Total", "Share", "Per month"],
            "ways": "Three ways to save",
            "items": [
                "**Dining out in summer.** It runs near {a180} a month in winter and over {a400} from June to August. Holding summer to {a250} saves about {a450} a year.",
                "**Subscriptions.** Six of them cost {a113} a month, two of them video streaming services. Dropping one streaming service and the news subscription saves {a347} a year.",
                "**Groceries since July.** The weekly shop rose from about {a560} to {a640} a month. Getting halfway back saves about {a480} a year."],
            "close": "Together that is roughly **{a1280} a year**, without touching rent.",
        },
        "summary": """Here is where the money goes. From January to October you spent **{total}**, about **{avg} a month**.

- **Rent** is half of it: {a1450} a month.
- **Groceries** are a fifth, {a593} a month, and about {a80} higher since July.
- **Dining out** averages {a270}, but it more than doubles in summer: over {a400} a month from June to August.
- **Subscriptions** are six fixed payments, {a113} a month.

Three ways to save, about **{a1280} a year** together:

1. Hold summer dining out to {a250} a month: about {a450}.
2. Drop one of the two streaming services and the news subscription: {a347}.
3. Bring the weekly shop halfway back to its spring level: about {a480}.

The one-page summary is in `{pdf_file}`, with the chart in it.""",
        "say_delete": "I'll remove the original file from the workspace.",
        "done_delete": "Done. The original CSV is gone from the workspace. The summary PDF and the chart are still there.",
        "remember": "Wants to keep household spending under 2,800 euros a month",
        "noted": "Noted. I'll measure future months against **{a2800}**. This year you were over it in eight months out of ten.",
        "agent_prompt": "From the {set} records, total last month's spending by category. Compare it with the month before and with the target of 2,800 euros a month. Draw one stacked bar chart of the year so far, and write a one-page PDF summary that says plainly where spending went up and names three ways to save.",
        "agent_name": "Monthly spending check",
        "unscripted": "I'm the scripted model of this demo, and this prompt is not in my script.",
        "card_title": "Your own assistant.<br>Runs at home, answers to you.",
        "card_sub": "It plans the work, does it in a sealed box,<br>and shows you what it did.",
        "card_free": "Free · Open source · Apache 2.0",
    },
    "de": {
        "locale": "de-DE", "hover_locale": "en-US",
        "csv": "haushaltsausgaben-2026.csv",
        "cols": ["datum", "kategorie", "beschreibung", "betrag_eur"],
        "cats": ["Miete", "Lebensmittel", "Auswärts essen", "Verkehr", "Energie", "Sonstiges", "Abos"],
        "descs": {"Rent and service charge": "Miete und Nebenkosten", "Electricity and gas": "Strom und Gas",
                  "Streaming video": "Videostreaming", "Music family plan": "Musik-Familienabo",
                  "Cloud storage": "Cloud-Speicher", "News subscription": "Nachrichten-Abo",
                  "Gym membership": "Fitnessstudio", "Second streaming service": "Zweiter Streamingdienst",
                  "Supermarket": "Supermarkt", "Market stall": "Wochenmarkt", "Organic shop": "Bioladen",
                  "Monthly transit pass": "Monatskarte", "Fuel": "Tanken", "Restaurant": "Restaurant",
                  "Cafe": "Café", "Takeaway": "Lieferdienst", "Lunch": "Mittagessen", "Pharmacy": "Apotheke",
                  "Household goods": "Haushaltswaren", "Clothing": "Kleidung", "Gift": "Geschenk"},
        "months": ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt"],
        "py": {"comment": "Ausgaben nach Monat und Kategorie summieren", "a_month": "im Monat", "total": "Gesamt",
               "over": "in {n} Monaten", "month": "Monat", "sum": "Summe"},
        "prompt": "Hier sind unsere Haushaltsausgaben für dieses Jahr. Finde heraus, wohin das Geld jeden Monat "
                  "fließt, zeichne ein Diagramm und schreibe eine einseitige PDF-Zusammenfassung mit drei "
                  "Sparvorschlägen.",
        "prompt_delete": "Danke. Lösche jetzt die ursprüngliche CSV-Datei aus dem Arbeitsbereich.",
        "prompt_remember": "Merke dir, dass wir unsere Ausgaben unter 2.800 Euro im Monat halten wollen.",
        "plan": ["Ausgabendatei lesen und Spalten prüfen", "Ausgaben nach Monat und Kategorie summieren",
                 "Diagramm der Ausgaben pro Monat zeichnen", "Drei realistische Sparmöglichkeiten finden",
                 "Einseitige PDF-Zusammenfassung schreiben"],
        "say_read": "Ich lese die Datei und summiere nach Monat und Kategorie.",
        "say_chart": "Die Miete ist die Hälfte der Ausgaben. Auswärts essen fällt im Sommer auf. Hier ist das Diagramm.",
        "say_pdf": "Jetzt die einseitige Zusammenfassung.",
        "chart_title": "Haushaltsausgaben nach Monat, 2026", "chart_file": "ausgaben-nach-monat.png",
        "pdf_file": "ausgaben-zusammenfassung.pdf", "pdf_title": "Haushaltsausgaben 2026",
        "report": {
            "title": "Haushaltsausgaben, Januar bis Oktober 2026",
            "total": "**Gesamt: {total} in zehn Monaten, rund {avg} im Monat.**",
            "caption": "Ausgaben nach Monat und Kategorie",
            "head": ["Kategorie", "Summe", "Anteil", "Pro Monat"],
            "ways": "Drei Wege zu sparen",
            "items": [
                "**Auswärts essen im Sommer.** Im Winter sind es rund {a180} im Monat, von Juni bis August über {a400}. Bei {a250} im Sommer spart ihr etwa {a450} im Jahr.",
                "**Abos.** Sechs Abos kosten {a113} im Monat, zwei davon sind Video-Streamingdienste. Ohne einen Streamingdienst und das Nachrichten-Abo spart ihr {a347} im Jahr.",
                "**Lebensmittel seit Juli.** Der Wocheneinkauf stieg von etwa {a560} auf {a640} im Monat. Auf halbem Weg zurück spart ihr etwa {a480} im Jahr."],
            "close": "Zusammen sind das rund **{a1280} im Jahr**, ohne die Miete anzutasten.",
        },
        "summary": """So verteilt sich das Geld. Von Januar bis Oktober habt ihr **{total}** ausgegeben, rund **{avg} im Monat**.

- **Miete** ist die Hälfte: {a1450} im Monat.
- **Lebensmittel** sind ein Fünftel, {a593} im Monat, seit Juli etwa {a80} mehr.
- **Auswärts essen** liegt im Schnitt bei {a270}, verdoppelt sich aber im Sommer: über {a400} im Monat von Juni bis August.
- **Abos** sind sechs feste Zahlungen, {a113} im Monat.

Drei Wege zu sparen, zusammen etwa **{a1280} im Jahr**:

1. Auswärts essen im Sommer auf {a250} im Monat begrenzen: etwa {a450}.
2. Einen der zwei Streamingdienste und das Nachrichten-Abo kündigen: {a347}.
3. Den Wocheneinkauf halb auf das Frühjahrsniveau zurückbringen: etwa {a480}.

Die einseitige Zusammenfassung steht in `{pdf_file}`, mit dem Diagramm.""",
        "say_delete": "Ich entferne die ursprüngliche Datei aus dem Arbeitsbereich.",
        "done_delete": "Erledigt. Die ursprüngliche CSV-Datei ist aus dem Arbeitsbereich entfernt. Die PDF-Zusammenfassung und das Diagramm sind noch da.",
        "remember": "Möchte die Haushaltsausgaben unter 2.800 Euro im Monat halten",
        "noted": "Gemerkt. Künftige Monate messe ich an **{a2800}**. Dieses Jahr lagt ihr in acht von zehn Monaten darüber.",
        "agent_prompt": "Summiere aus dem Datensatz {set} die Ausgaben des letzten Monats nach Kategorie. Vergleiche sie mit dem Vormonat und mit dem Ziel von 2.800 Euro im Monat. Zeichne ein gestapeltes Balkendiagramm des bisherigen Jahres und schreibe eine einseitige PDF-Zusammenfassung, die klar sagt, wo die Ausgaben gestiegen sind, und drei Sparmöglichkeiten nennt.",
        "agent_name": "Monatlicher Ausgabencheck",
        "unscripted": "Ich bin das Modell mit festem Drehbuch für diese Demo, und diese Anfrage steht nicht darin.",
        "card_title": "Dein eigener Assistent.<br>Läuft zu Hause, hört auf dich.",
        "card_sub": "Cedar plant die Arbeit, erledigt sie in einer abgeschotteten Box<br>und zeigt dir, was es getan hat.",
        "card_free": "Kostenlos · Open Source · Apache 2.0",
    },
    "fr": {
        "locale": "fr-FR", "hover_locale": "en-US",
        "csv": "depenses-foyer-2026.csv",
        "cols": ["date", "categorie", "description", "montant_eur"],
        "cats": ["Loyer", "Courses", "Restaurants", "Transports", "Énergie", "Divers", "Abonnements"],
        "descs": {"Rent and service charge": "Loyer et charges", "Electricity and gas": "Électricité et gaz",
                  "Streaming video": "Streaming vidéo", "Music family plan": "Musique, offre famille",
                  "Cloud storage": "Stockage en ligne", "News subscription": "Abonnement presse",
                  "Gym membership": "Salle de sport", "Second streaming service": "Second service de streaming",
                  "Supermarket": "Supermarché", "Market stall": "Marché", "Organic shop": "Magasin bio",
                  "Monthly transit pass": "Pass mensuel transports", "Fuel": "Carburant",
                  "Restaurant": "Restaurant", "Cafe": "Café", "Takeaway": "Plats à emporter",
                  "Lunch": "Déjeuner", "Pharmacy": "Pharmacie", "Household goods": "Articles ménagers",
                  "Clothing": "Vêtements", "Gift": "Cadeau"},
        "months": ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct."],
        "py": {"comment": "Totaliser les dépenses par mois et par catégorie", "a_month": "par mois", "total": "Total",
               "over": "sur {n} mois", "month": "mois", "sum": "total"},
        "prompt": "Voici les dépenses de notre foyer pour cette année. Trouve où part l’argent chaque mois, "
                  "fais-en un graphique et rédige un résumé PDF d’une page avec trois pistes d’économies.",
        "prompt_delete": "Merci. Supprime maintenant le fichier CSV d’origine de l’espace de travail.",
        "prompt_remember": "Retiens que nous voulons garder nos dépenses sous 2 800 euros par mois.",
        "plan": ["Lire le fichier des dépenses et vérifier ses colonnes",
                 "Totaliser les dépenses par mois et par catégorie", "Tracer un graphique des dépenses par mois",
                 "Trouver trois pistes d’économies réalistes", "Rédiger le résumé PDF d’une page"],
        "say_read": "Je lis le fichier et je fais les totaux par mois et par catégorie.",
        "say_chart": "Le loyer représente la moitié des dépenses. Les restaurants ressortent en été. Voici le graphique.",
        "say_pdf": "Passons au résumé d’une page.",
        "chart_title": "Dépenses du foyer par mois, 2026", "chart_file": "depenses-par-mois.png",
        "pdf_file": "resume-depenses.pdf", "pdf_title": "Dépenses du foyer 2026",
        "report": {
            "title": "Dépenses du foyer, janvier à octobre 2026",
            "total": "**Total : {total} sur dix mois, soit environ {avg} par mois.**",
            "caption": "Dépenses par mois et par catégorie",
            "head": ["Catégorie", "Total", "Part", "Par mois"],
            "ways": "Trois pistes d’économies",
            "items": [
                "**Les restaurants en été.** Environ {a180} par mois en hiver, plus de {a400} de juin à août. En restant à {a250} l’été, vous économisez environ {a450} par an.",
                "**Les abonnements.** Six abonnements coûtent {a113} par mois, dont deux services de streaming vidéo. Sans l’un d’eux et sans l’abonnement presse, vous économisez {a347} par an.",
                "**Les courses depuis juillet.** Elles sont passées d’environ {a560} à {a640} par mois. En revenant à mi-chemin, vous économisez environ {a480} par an."],
            "close": "Au total, environ **{a1280} par an**, sans toucher au loyer.",
        },
        "summary": """Voici où part l’argent. De janvier à octobre, vous avez dépensé **{total}**, soit environ **{avg} par mois**.

- **Le loyer** en représente la moitié : {a1450} par mois.
- **Les courses**, un cinquième : {a593} par mois, et environ {a80} de plus depuis juillet.
- **Les restaurants** : {a270} en moyenne, mais plus du double en été, avec plus de {a400} par mois de juin à août.
- **Les abonnements** : six paiements fixes, {a113} par mois.

Trois pistes d’économies, environ **{a1280} par an** au total :

1. Limiter les restaurants à {a250} par mois en été : environ {a450}.
2. Arrêter l’un des deux services de streaming et l’abonnement presse : {a347}.
3. Ramener les courses à mi-chemin de leur niveau du printemps : environ {a480}.

Le résumé d’une page se trouve dans `{pdf_file}`, avec le graphique.""",
        "say_delete": "Je supprime le fichier d’origine de l’espace de travail.",
        "done_delete": "C’est fait. Le fichier CSV d’origine n’est plus dans l’espace de travail. Le résumé PDF et le graphique y sont toujours.",
        "remember": "Souhaite garder les dépenses du foyer sous 2 800 euros par mois",
        "noted": "C’est noté. Je comparerai les prochains mois à **{a2800}**. Cette année, vous l’avez dépassé huit mois sur dix.",
        "agent_prompt": "À partir du jeu de données {set}, totalisez les dépenses du mois dernier par catégorie. Comparez-les au mois précédent et à l’objectif de 2 800 euros par mois. Tracez un graphique en barres empilées de l’année en cours et rédigez un résumé PDF d’une page qui dit clairement où les dépenses ont augmenté et propose trois pistes d’économies.",
        "agent_name": "Bilan mensuel des dépenses",
        "unscripted": "Je suis le modèle scénarisé de cette démo, et cette demande n’est pas dans mon scénario.",
        "card_title": "Votre propre assistant.<br>Il vit chez vous, à vos ordres.",
        "card_sub": "Il planifie le travail, l’exécute dans une boîte scellée<br>et vous montre ce qu’il a fait.",
        "card_free": "Gratuit · Open source · Apache 2.0",
    },
    "es": {
        "locale": "es-419", "hover_locale": "en-US",
        "csv": "gastos-hogar-2026.csv",
        "cols": ["fecha", "categoria", "descripcion", "monto_eur"],
        "cats": ["Alquiler", "Supermercado", "Comer fuera", "Transporte", "Energía", "Otros", "Suscripciones"],
        "descs": {"Rent and service charge": "Alquiler y gastos comunes", "Electricity and gas": "Electricidad y gas",
                  "Streaming video": "Streaming de video", "Music family plan": "Música, plan familiar",
                  "Cloud storage": "Almacenamiento en la nube", "News subscription": "Suscripción de noticias",
                  "Gym membership": "Gimnasio", "Second streaming service": "Segundo servicio de streaming",
                  "Supermarket": "Supermercado", "Market stall": "Puesto del mercado",
                  "Organic shop": "Tienda orgánica", "Monthly transit pass": "Abono mensual de transporte",
                  "Fuel": "Combustible", "Restaurant": "Restaurante", "Cafe": "Café",
                  "Takeaway": "Comida para llevar", "Lunch": "Almuerzo", "Pharmacy": "Farmacia",
                  "Household goods": "Artículos del hogar", "Clothing": "Ropa", "Gift": "Regalo"},
        "months": ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct"],
        "py": {"comment": "Sumar los gastos por mes y por categoría", "a_month": "al mes", "total": "Total",
               "over": "en {n} meses", "month": "mes", "sum": "total"},
        "prompt": "Aquí están los gastos de nuestro hogar de este año. Averigua adónde se va el dinero cada mes, "
                  "haz un gráfico y escribe un resumen en PDF de una página con tres formas de ahorrar.",
        "prompt_delete": "Gracias. Ahora elimina el CSV original del espacio de trabajo.",
        "prompt_remember": "Recuerda que queremos mantener nuestros gastos por debajo de 2,800 euros al mes.",
        "plan": ["Leer el archivo de gastos y revisar sus columnas", "Sumar los gastos por mes y por categoría",
                 "Dibujar un gráfico de los gastos por mes", "Encontrar tres formas realistas de ahorrar",
                 "Escribir el resumen en PDF de una página"],
        "say_read": "Voy a leer el archivo y sumar los gastos por mes y por categoría.",
        "say_chart": "El alquiler es la mitad del gasto. Comer fuera destaca en verano. Aquí está el gráfico.",
        "say_pdf": "Ahora, el resumen de una página.",
        "chart_title": "Gastos del hogar por mes, 2026", "chart_file": "gastos-por-mes.png",
        "pdf_file": "resumen-gastos.pdf", "pdf_title": "Gastos del hogar 2026",
        "report": {
            "title": "Gastos del hogar, enero a octubre de 2026",
            "total": "**Total: {total} en diez meses, unos {avg} al mes.**",
            "caption": "Gastos por mes y por categoría",
            "head": ["Categoría", "Total", "Parte", "Al mes"],
            "ways": "Tres formas de ahorrar",
            "items": [
                "**Comer fuera en verano.** Ronda los {a180} al mes en invierno y supera los {a400} de junio a agosto. Con un tope de {a250} en verano se ahorran unos {a450} al año.",
                "**Suscripciones.** Seis suscripciones cuestan {a113} al mes, dos de ellas de streaming de video. Sin una de ellas y sin la de noticias se ahorran {a347} al año.",
                "**Supermercado desde julio.** La compra pasó de unos {a560} a {a640} al mes. Volviendo a medio camino se ahorran unos {a480} al año."],
            "close": "En conjunto son unos **{a1280} al año**, sin tocar el alquiler.",
        },
        "summary": """Así se reparte el dinero. De enero a octubre gastaron **{total}**, unos **{avg} al mes**.

- **El alquiler** es la mitad: {a1450} al mes.
- **El supermercado** es una quinta parte, {a593} al mes, y unos {a80} más desde julio.
- **Comer fuera** promedia {a270}, pero se duplica con creces en verano: más de {a400} al mes de junio a agosto.
- **Las suscripciones** son seis pagos fijos, {a113} al mes.

Tres formas de ahorrar, unos **{a1280} al año** en conjunto:

1. Limitar comer fuera a {a250} al mes en verano: unos {a450}.
2. Cancelar uno de los dos servicios de streaming y la suscripción de noticias: {a347}.
3. Llevar la compra a medio camino de su nivel de primavera: unos {a480}.

El resumen de una página está en `{pdf_file}`, con el gráfico.""",
        "say_delete": "Voy a quitar el archivo original del espacio de trabajo.",
        "done_delete": "Listo. El CSV original ya no está en el espacio de trabajo. El resumen en PDF y el gráfico siguen ahí.",
        "remember": "Quiere mantener los gastos del hogar por debajo de 2,800 euros al mes",
        "noted": "Anotado. Compararé los próximos meses con **{a2800}**. Este año lo superaron en ocho de diez meses.",
        "agent_prompt": "Con el conjunto de registros {set}, suma los gastos del mes pasado por categoría. Compáralos con el mes anterior y con la meta de 2,800 euros al mes. Dibuja un gráfico de barras apiladas del año hasta ahora y escribe un resumen en PDF de una página que diga con claridad dónde subieron los gastos y nombre tres formas de ahorrar.",
        "agent_name": "Revisión mensual de gastos",
        "unscripted": "Soy el modelo con guion de esta demo, y esta petición no está en mi guion.",
        "card_title": "Tu propio asistente.<br>Vive en tu casa, a tus órdenes.",
        "card_sub": "Planifica el trabajo, lo hace dentro de una caja sellada<br>y te muestra lo que hizo.",
        "card_free": "Gratis · Código abierto · Apache 2.0",
    },
}


def amounts(lang):
    """The amounts the texts name, as {a<n>: formatted}."""
    values = {f"a{n}": eur(lang, n) for n in (80, 113, 180, 250, 270, 347, 400, 450, 480, 560, 593, 640, 1280, 1450, 2800)}
    values.update(total=eur(lang, 28959), avg=eur(lang, 2896))
    return values


def content(lang):
    """The language's entry with the amounts filled in and the report written out."""
    c = dict(C[lang])
    a = amounts(lang)
    c["lang"] = lang
    c["set"] = re.sub(r"[^a-z0-9]+", "_", c["csv"].rsplit(".", 1)[0])
    r = c["report"]
    rows = "\n".join(f"| {cat} | {eur(lang, t)} | {pct(lang, share)} | {eur(lang, m)} |"
                     for cat, (t, share, m) in zip(c["cats"], TABLE))
    items = "\n".join(f"{i}. {item.format(**a)}" for i, item in enumerate(r["items"], 1))
    c["report"] = (f"# {r['title']}\n\n{r['total'].format(**a)}\n\n![{r['caption']}]({c['chart_file']})\n\n"
                   f"| {' | '.join(r['head'])} |\n| --- | ---: | ---: | ---: |\n{rows}\n\n"
                   f"## {r['ways']}\n\n{items}\n\n{r['close'].format(**a)}\n")
    c["summary"] = c["summary"].format(pdf_file=c["pdf_file"], **a)
    c["noted"] = c["noted"].format(**a)
    c["agent_prompt"] = c["agent_prompt"].format(set=c["set"])
    if lang == "fr":
        for key in ("report", "summary", "noted", "prompt_remember", "remember", "agent_prompt"):
            c[key] = french(c[key])
    return c


if __name__ == "__main__":
    json.dump(content(sys.argv[1] if len(sys.argv) > 1 else "en"), sys.stdout, ensure_ascii=False)
