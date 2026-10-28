// What's new (shown once after a deploy that brings an entry the player hasn't
// seen; settings → "Neuigkeiten" shows them again). Newest first; `rules`:
// the game rules changed. New players start with all of it seen.
import { lang } from "./i18n";
import { $ } from "./page";
import { KEYS, load, save } from "./store";
import { activeNotices, text as noticeText } from "./notices";
import { paragraphs } from "./story";
import { withRegion } from "./region";

type Text = { title: string; items: string[] };
type Entry = { id: string; rules?: boolean; text: Record<string, Text> };

export const NEWS: Entry[] = [
  {
    id: "2026-10-28",
    text: {
      de: {
        title: "Neu: Freunde einladen",
        items: [
          "Im Profil gibt es jetzt „Freunde einladen“: Teil deinen Link übers Handy, kopier ihn oder zeig ihn als QR-Code, etwa jemandem, der neben dir steht.",
          "Steigt jemand über deinen Link neu ein, erobert mindestens fünf Kreuzungen und spielt an einem späteren Tag wieder, bekommst du 200 Punkte. Bis zu zehn Einladungen in 30 Tagen zählen.",
        ],
      },
      en: {
        title: "New: invite friends",
        items: [
          "Your profile now has “Invite friends”: share your link from your phone, copy it or show it as a QR code, for example to someone standing next to you.",
          "If someone joins through your link, claims at least five junctions and plays again on a later day, you get 200 points. Up to ten invites in 30 days count.",
        ],
      },
      fr: {
        title: "Nouveau&nbsp;: inviter des amis",
        items: [
          "Ton profil propose maintenant «&nbsp;Inviter des amis&nbsp;»&nbsp;: partage ton lien depuis ton téléphone, copie-le ou montre-le en code QR, par exemple à quelqu’un à côté de toi.",
          "Si quelqu’un arrive par ton lien, conquiert au moins cinq carrefours et rejoue un autre jour, tu gagnes 200 points. Jusqu’à dix invitations sur 30 jours comptent.",
        ],
      },
      es: {
        title: "Novedad: invita a tus amigos",
        items: [
          "En tu perfil está ahora «Invitar a amigos»: comparte tu enlace desde el móvil, cópialo o muéstralo como código QR, por ejemplo a quien esté a tu lado.",
          "Si alguien entra con tu enlace, conquista al menos cinco cruces y vuelve a jugar otro día, recibes 200 puntos. Cuentan hasta diez invitaciones en 30 días.",
        ],
      },
      pl: {
        title: "Nowość: zaproś znajomych",
        items: [
          "W profilu jest teraz „Zaproś znajomych”: udostępnij swój link z telefonu, skopiuj go albo pokaż jako kod QR, na przykład komuś, kto stoi obok.",
          "Jeśli ktoś dołączy przez twój link, zdobędzie co najmniej pięć skrzyżowań i zagra ponownie innego dnia, dostajesz 200 pkt. Liczy się do dziesięciu zaproszeń w ciągu 30 dni.",
        ],
      },
      it: {
        title: "Novità: invita gli amici",
        items: [
          "Nel profilo c’è ora «Invita amici»: condividi il tuo link dal telefono, copialo o mostralo come codice QR, per esempio a chi ti sta accanto.",
          "Se qualcuno entra con il tuo link, conquista almeno cinque incroci e torna a giocare un altro giorno, ricevi 200 punti. Contano fino a dieci inviti in 30 giorni.",
        ],
      },
      pt: {
        title: "Novidade: convide amigos",
        items: [
          "No seu perfil agora há “Convidar amigos”: compartilhe seu link pelo celular, copie ou mostre como QR code, por exemplo para quem está ao seu lado.",
          "Se alguém entrar pelo seu link, conquistar pelo menos cinco cruzamentos e jogar de novo em outro dia, você ganha 200 pontos. Contam até dez convites em 30 dias.",
        ],
      },
    },
  },
  {
    id: "2026-10-27",
    rules: true,
    text: {
      de: {
        title: "Neu: Bilder, verbundene Glut und Eis, sprudelnde Quellen",
        items: [
          "Erobern sieht jetzt anders aus: Feuer schießt die Straßen bergauf, Wasser fließt bergab, vom eroberten Ort aus. Die Eroberungen der anderen blitzen auch auf, und deine Glut trägt eine Flamme, dein Eis einen Kristall.",
          "Die Quelle sprudelt, die Glut erwacht: Bleib an einer Senke (Wasser) oder auf einer Kuppe (Feuer) eine Minute stehen. Dann steigt dort eine Quelle auf, und mit ihr die ganze Senke bzw. Kuppe, bis zu acht Kreuzungen.",
          "Glut und Eis verbinden: Zwei deiner Glutquellen bzw. Eisquellen kannst du verbinden („Verbinden …“ auf der Karte der Quelle). Stehst du an der einen, eroberst du mit „Drüben erobern“ rund um die andere, egal wie weit sie weg ist.",
          "Kapitel mit Geschichte: Kampagnen erzählen, was passiert, und manche Fundstücke sind Briefe. Was war, steht in der Chronik; im Album öffnet jedes Fundstück sein Bild wieder.",
        ],
      },
      en: {
        title: "New: visuals, linked embers and ice, rising springs",
        items: [
          "Claiming looks different now: fire races uphill along the streets, water flows downhill, from the place you claimed. Other players’ claims flash too, and your embers carry a flame, your ice a crystal.",
          "The spring rises, the embers wake: stay a minute at a hollow (water) or on a hilltop (fire). A source rises there, and with it the whole hollow or hilltop, up to eight junctions.",
          "Link embers and ice: you can link two of your embers or two of your ice (“Link …” on the source’s card). Standing at one, “Claim over there” claims around the other, however far away it is.",
          "Chapters with a story: campaigns tell what happens, and some finds are letters. What happened is in the chronicle; in the album every find opens its picture again.",
        ],
      },
      fr: {
        title: "Nouveau : images, braises et glace reliées, sources qui jaillissent",
        items: [
          "Conquérir a changé d’allure&nbsp;: le feu file vers le haut des rues, l’eau coule vers le bas, depuis le lieu conquis. Les conquêtes des autres scintillent aussi, et tes braises portent une flamme, ta glace un cristal.",
          "La source jaillit, les braises s’éveillent&nbsp;: reste une minute dans un creux (eau) ou sur une butte (feu). Une source y jaillit, et avec elle tout le creux ou toute la butte, jusqu’à huit carrefours.",
          "Relier braises et glace&nbsp;: tu peux relier deux de tes braises ou deux de tes glaces («&nbsp;Relier …&nbsp;» sur la fiche de la source). Devant l’une, «&nbsp;Conquérir là-bas&nbsp;» conquiert autour de l’autre, quelle que soit la distance.",
          "Des chapitres avec une histoire&nbsp;: les campagnes racontent ce qui se passe, et certaines trouvailles sont des lettres. Ce qui s’est passé est dans la chronique&nbsp;; dans l’album, chaque trouvaille rouvre son image.",
        ],
      },
      es: {
        title: "Novedades: imágenes, brasas y hielo enlazados, manantiales que brotan",
        items: [
          "Conquistar se ve distinto: el fuego sube corriendo por las calles, el agua baja, desde el lugar conquistado. Las conquistas de los demás también destellan, y tus brasas llevan una llama, tu hielo un cristal.",
          "Brota el manantial, despiertan las brasas: quédate un minuto en una hondonada (agua) o en una loma (fuego). Allí brota una fuente, y con ella toda la hondonada o la loma, hasta ocho cruces.",
          "Enlazar brasas y hielo: puedes enlazar dos de tus brasas o dos de tus hielos («Enlazar …» en la ficha de la fuente). Junto a una, «Conquistar allá» conquista alrededor de la otra, esté donde esté.",
          "Capítulos con historia: las campañas cuentan lo que pasa, y algunos hallazgos son cartas. Lo ocurrido está en la crónica; en el álbum cada hallazgo vuelve a abrir su imagen.",
        ],
      },
      pl: {
        title: "Nowości: obrazy, połączony żar i lód, tryskające źródła",
        items: [
          "Zdobywanie wygląda teraz inaczej: ogień pędzi ulicami pod górę, woda spływa w dół, od zdobytego miejsca. Zdobycze innych też błyskają, a twój żar nosi płomień, twój lód kryształ.",
          "Źródło tryska, żar się budzi: postój minutę w zagłębieniu (woda) albo na wzniesieniu (ogień). Wytryska tam źródło, a z nim całe zagłębienie albo wzniesienie, do ośmiu skrzyżowań.",
          "Połącz żar i lód: dwa swoje żary albo dwa lody możesz połączyć („Połącz …” na karcie źródła). Stojąc przy jednym, „Zdobądź tam” zdobywa wokół drugiego, bez względu na odległość.",
          "Rozdziały z historią: kampanie opowiadają, co się dzieje, a niektóre znaleziska to listy. To, co było, jest w kronice; w albumie każde znalezisko znów otwiera swój obraz.",
        ],
      },
      it: {
        title: "Novità: immagini, braci e ghiaccio collegati, sorgenti che sgorgano",
        items: [
          "Conquistare ha un altro aspetto: il fuoco corre in salita lungo le strade, l’acqua scorre in discesa, dal posto conquistato. Anche le conquiste degli altri brillano, e le tue braci portano una fiamma, il tuo ghiaccio un cristallo.",
          "La sorgente sgorga, le braci si svegliano: resta un minuto in un avvallamento (acqua) o su un dosso (fuoco). Lì sorge una fonte, e con essa tutto l’avvallamento o il dosso, fino a otto incroci.",
          "Collega braci e ghiaccio: puoi collegare due tue braci o due tuoi ghiacci («Collega …» sulla scheda della fonte). Vicino a una, «Conquista laggiù» conquista intorno all’altra, per quanto lontana sia.",
          "Capitoli con una storia: le campagne raccontano cosa succede, e alcuni ritrovamenti sono lettere. Quel che è stato è nella cronaca; nell’album ogni ritrovamento riapre la sua immagine.",
        ],
      },
      pt: {
        title: "Novidades: imagens, brasas e gelo ligados, nascentes que brotam",
        items: [
          "Conquistar está diferente: o fogo sobe a correr pelas ruas, a água desce, a partir do sítio conquistado. As conquistas dos outros também brilham, e as tuas brasas levam uma chama, o teu gelo um cristal.",
          "A nascente brota, as brasas despertam: fica um minuto numa depressão (água) ou numa colina (fogo). Brota lá uma fonte, e com ela toda a depressão ou colina, até oito cruzamentos.",
          "Ligar brasas e gelo: podes ligar duas das tuas brasas ou dois dos teus gelos («Ligar …» na ficha da fonte). Junto de uma, «Conquistar lá» conquista à volta da outra, esteja onde estiver.",
          "Capítulos com história: as campanhas contam o que acontece, e alguns achados são cartas. O que aconteceu está na crónica; no álbum cada achado volta a abrir a sua imagem.",
        ],
      },
    },
  },
  {
    id: "2026-10-11",
    rules: true,
    text: {
      de: {
        title: "Gezeiten in Hamburg",
        items: [
          "Gezeiten live: In Hamburg zählt jetzt der Wasserstand der Elbe (Pegel St. Pauli). Bei Flut haben es bis etwa 400&nbsp;m vom Wasser die natürlichen Feuerquellen schwerer, bei Ebbe die Wasserquellen. Das 🌊 neben dem Wetter zeigt den Stand.",
          "Bitte Abstand zum Wasser halten: Anleger, Uferkanten und Watt sind kein Spielfeld, und am Ufer selbst setzt das Spiel keine Ziele.",
        ],
      },
      en: {
        title: "Tides in Hamburg",
        items: [
          "Live tide: in Hamburg the level of the Elbe now counts (St. Pauli gauge). At high tide, up to about 400&nbsp;m from the water, natural fire sources have a harder time; at low tide the water sources do. The 🌊 next to the weather shows the state.",
          "Please keep away from the water: piers, quay edges and mudflats are no playing field, and the game sets no targets at the shore itself.",
        ],
      },
      fr: {
        title: "Marées à Hambourg",
        items: [
          "Marée en direct&nbsp;: à Hambourg, le niveau de l’Elbe compte maintenant (échelle de Sankt Pauli). À marée haute, jusqu’à environ 400&nbsp;m de l’eau, les sources de feu naturelles ont plus de mal&nbsp;; à marée basse, ce sont les sources d’eau. Le 🌊 à côté de la météo montre l’état.",
          "Garde tes distances avec l’eau&nbsp;: pontons, bords de quai et estran ne font pas partie du jeu, et le jeu ne place aucun objectif sur la rive même.",
        ],
      },
      es: {
        title: "Mareas en Hamburgo",
        items: [
          "Marea en directo: en Hamburgo ahora cuenta el nivel del Elba (mareógrafo de St. Pauli). Con marea alta, hasta unos 400&nbsp;m del agua, las fuentes de fuego naturales lo tienen más difícil; con marea baja, las de agua. El 🌊 junto al tiempo muestra el estado.",
          "Mantente lejos del agua: muelles, bordes del malecón y zonas de marea no son terreno de juego, y el juego no pone objetivos en la orilla misma.",
        ],
      },
      pl: {
        title: "Pływy w Hamburgu",
        items: [
          "Pływy na żywo: w Hamburgu liczy się teraz poziom Łaby (wodowskaz St. Pauli). Przy przypływie, do około 400&nbsp;m od wody, naturalne źródła ognia mają trudniej, przy odpływie źródła wody. 🌊 obok pogody pokazuje stan.",
          "Trzymaj się z dala od wody: pomosty, krawędzie nabrzeża i watty nie są polem gry, a na samym brzegu gra nie wyznacza celów.",
        ],
      },
      it: {
        title: "Maree ad Amburgo",
        items: [
          "Marea dal vivo: ad Amburgo ora conta il livello dell’Elba (mareografo di St. Pauli). Con l’alta marea, fino a circa 400&nbsp;m dall’acqua, le fonti di fuoco naturali fanno più fatica; con la bassa marea quelle d’acqua. Il 🌊 accanto al meteo mostra lo stato.",
          "Stai lontano dall’acqua: pontili, bordi delle banchine e zone di marea non fanno parte del gioco, e sulla riva stessa il gioco non mette obiettivi.",
        ],
      },
      pt: {
        title: "Marés em Hamburgo",
        items: [
          "Maré ao vivo: em Hamburgo agora conta o nível do Elba (marégrafo de St. Pauli). Na maré alta, até uns 400&nbsp;m da água, as fontes de fogo naturais têm mais dificuldade; na maré baixa, as de água. O 🌊 ao lado do tempo mostra o estado.",
          "Fique longe da água: píeres, bordas de cais e áreas de maré não são campo de jogo, e na própria margem o jogo não coloca alvos.",
        ],
      },
    },
  },
  {
    id: "2026-10-10c",
    text: {
      de: {
        title: "Globus und Portugiesisch",
        items: [
          "Unten in der Leiste führt „Globus“ zum Spielstand weltweit auf dem Globus von Weathermarble (externe Seite).",
          "Glut &amp; Flut gibt es jetzt auch auf Portugiesisch.",
        ],
      },
      en: {
        title: "Globe and Portuguese",
        items: [
          "“Globe” in the bottom line leads to the game worldwide on the Weathermarble globe (external site).",
          "Glut &amp; Flut is now also available in Portuguese.",
        ],
      },
      fr: {
        title: "Globe et portugais",
        items: [
          "«&nbsp;Globe&nbsp;» dans la barre du bas mène à la partie dans le monde entier sur le globe de Weathermarble (site externe).",
          "Glut &amp; Flut existe maintenant aussi en portugais.",
        ],
      },
      es: {
        title: "Globo y portugués",
        items: [
          "«Globo», en la barra inferior, lleva al juego en todo el mundo en el globo de Weathermarble (sitio externo).",
          "Glut &amp; Flut ya está también en portugués.",
        ],
      },
      pl: {
        title: "Globus i portugalski",
        items: [
          "„Globus” na dolnym pasku prowadzi do stanu gry na całym świecie na globusie Weathermarble (strona zewnętrzna).",
          "Glut &amp; Flut jest teraz dostępny także po portugalsku.",
        ],
      },
      it: {
        title: "Globo e portoghese",
        items: [
          "«Globo» nella barra in basso porta alla partita in tutto il mondo sul globo di Weathermarble (sito esterno).",
          "Glut &amp; Flut ora c’è anche in portoghese.",
        ],
      },
      pt: {
        title: "Globo e português",
        items: [
          "“Globo”, na barra de baixo, leva ao jogo no mundo inteiro no globo da Weathermarble (site externo).",
          "Glut &amp; Flut agora também está em português.",
        ],
      },
    },
  },
  {
    id: "2026-10-10b",
    rules: true,
    text: {
      de: {
        title: "Sonne live und Stadt-Serie",
        items: [
          "Sonne live: Klarer Himmel tagsüber hilft jetzt dem Feuer, genau dort, wo kaum Wolken sind (Wasser altert schneller, natürliche Wasserquellen sind schwächer). Das Gegenstück zum Regen.",
          "In der Rangliste steht jetzt, wie viele Tage in Folge in deiner Stadt mindestens fünf Leute Punkte geholt haben. Bringt Leute mit!",
        ],
      },
      en: {
        title: "Live sun and city streak",
        items: [
          "Live sun: a clear sky by day now helps fire, exactly where there are hardly any clouds (water ages faster, natural water sources are weaker). The counterpart to rain.",
          "The leaderboard now shows how many days in a row at least five people scored points in your city. Bring people along!",
        ],
      },
      fr: {
        title: "Soleil en direct et série de la ville",
        items: [
          "Soleil en direct&nbsp;: un ciel dégagé en journée aide maintenant le feu, exactement là où il y a peu de nuages (l’eau vieillit plus vite, les sources d’eau naturelles sont plus faibles). Le pendant de la pluie.",
          "Le classement montre maintenant combien de jours d’affilée au moins cinq personnes ont marqué des points dans ta ville. Amène du monde&nbsp;!",
        ],
      },
      es: {
        title: "Sol en directo y racha de la ciudad",
        items: [
          "Sol en directo: un cielo despejado de día ayuda ahora al fuego, justo donde apenas hay nubes (el agua envejece más rápido, las fuentes de agua naturales son más débiles). La contrapartida de la lluvia.",
          "La clasificación muestra ahora cuántos días seguidos al menos cinco personas han conseguido puntos en tu ciudad. ¡Trae gente!",
        ],
      },
      pl: {
        title: "Słońce na żywo i seria miasta",
        items: [
          "Słońce na żywo: czyste niebo za dnia pomaga teraz ogniowi, dokładnie tam, gdzie prawie nie ma chmur (woda starzeje się szybciej, naturalne źródła wody są słabsze). Odpowiednik deszczu.",
          "Ranking pokazuje teraz, ile dni z rzędu co najmniej pięć osób zdobyło punkty w twoim mieście. Przyprowadź znajomych!",
        ],
      },
      it: {
        title: "Sole dal vivo e serie della città",
        items: [
          "Sole dal vivo: un cielo sereno di giorno ora aiuta il fuoco, esattamente dove ci sono poche nuvole (l’acqua invecchia più in fretta, le sorgenti naturali d’acqua sono più deboli). Il contrario della pioggia.",
          "La classifica ora mostra per quanti giorni di fila almeno cinque persone hanno fatto punti nella tua città. Porta gente!",
        ],
      },
      pt: {
        title: "Sol ao vivo e sequência da cidade",
        items: [
          "Sol ao vivo: um céu limpo de dia agora ajuda o fogo, exatamente onde quase não há nuvens (a água envelhece mais rápido, as fontes naturais de água ficam mais fracas). O contrário da chuva.",
          "O ranking agora mostra quantos dias seguidos pelo menos cinco pessoas fizeram pontos na sua cidade. Traga a galera!",
        ],
      },
    },
  },
  {
    id: "2026-10-10",
    rules: true,
    text: {
      de: {
        title: "Wochenserie und Regen live",
        items: [
          "Spielst du an drei Tagen einer Woche, ist die Woche geschafft und deine Serie wächst (unter deinem Namen und in der Rangliste). Ein Spieltag: drei Eroberungen, eine Herausforderung, ein Fundstück oder eine Ortsbewertung. Abzeichen für 2, 4 und 8 Wochen.",
          "Eine verpasste Woche je Kampagne wird verziehen; zwischen Kampagnen ruht die Serie.",
          "Ist an deinem Standort schon alles eures, sagt dir „Erobern“ jetzt, wo die nächste Front liegt.",
          "Regen live: Wo es Regendaten gibt, zählt der Regen genau dort, wo es gerade regnet (Feuer altert schneller, natürliche Feuerquellen brennen schwächer), statt den ganzen Tag in der ganzen Stadt.",
          "Ist schon ein Herd oder Brunnen in der Nähe, stärkt „Erobern“ jetzt eine eigene Kreuzung direkt an der Front.",
        ],
      },
      en: {
        title: "Weekly streak and live rain",
        items: [
          "Play on three days of a week and the week is done, and your streak grows (under your name and in the leaderboard). A play day: three claims, a challenge, a find or a place rating. Badges for 2, 4 and 8 weeks.",
          "One missed week per campaign is forgiven; between campaigns the streak pauses.",
          "If everything around you is already yours, “Claim” now tells you where the nearest front is.",
          "Live rain: where there is rain data, rain counts exactly where it is raining now (fire ages faster, natural fire sources burn weaker) instead of all day across the whole city.",
          "If a hearth or well is already nearby, “Claim” now reinforces one of your junctions right at the front.",
        ],
      },
      fr: {
        title: "Série hebdomadaire et pluie en direct",
        items: [
          "Joue trois jours dans une semaine et la semaine est réussie&nbsp;: ta série grandit (sous ton nom et dans le classement). Un jour de jeu&nbsp;: trois conquêtes, un défi, une trouvaille ou un avis sur un lieu. Badges pour 2, 4 et 8&nbsp;semaines.",
          "Une semaine manquée par campagne est pardonnée&nbsp;; entre deux campagnes, la série est en pause.",
          "Si tout est déjà à toi autour de toi, «&nbsp;Conquérir&nbsp;» t’indique maintenant où est le front le plus proche.",
          "Pluie en direct&nbsp;: là où il y a des données de pluie, la pluie compte exactement là où il pleut (le feu vieillit plus vite, les sources de feu naturelles brûlent plus faiblement), au lieu de toute la journée dans toute la ville.",
          "S’il y a déjà un foyer ou un puits tout près, «&nbsp;Conquérir&nbsp;» renforce maintenant un de tes carrefours juste sur le front.",
        ],
      },
      es: {
        title: "Racha semanal y lluvia en directo",
        items: [
          "Si juegas tres días en una semana, la semana está lograda y tu racha crece (bajo tu nombre y en la clasificación). Un día de juego: tres conquistas, un desafío, un hallazgo o una valoración de un lugar. Insignias por 2, 4 y 8 semanas.",
          "Se perdona una semana perdida por campaña; entre campañas la racha descansa.",
          "Si a tu alrededor ya todo es vuestro, «Conquistar» te dice ahora dónde está el frente más cercano.",
          "Lluvia en directo: donde hay datos de lluvia, la lluvia cuenta justo donde está lloviendo (el fuego envejece más rápido, las fuentes de fuego naturales arden más débiles), en vez de todo el día en toda la ciudad.",
          "Si ya hay un hogar o un pozo cerca, «Conquistar» refuerza ahora un cruce tuyo justo en el frente.",
        ],
      },
      pl: {
        title: "Seria tygodniowa i deszcz na żywo",
        items: [
          "Grasz trzy dni w tygodniu – tydzień zaliczony, a twoja seria rośnie (pod twoim imieniem i w rankingu). Dzień gry: trzy zdobycia, wyzwanie, znalezisko lub ocena miejsca. Odznaki za 2, 4 i 8 tygodni.",
          "Jeden opuszczony tydzień na kampanię jest wybaczany; między kampaniami seria czeka.",
          "Jeśli wokół ciebie wszystko już jest wasze, „Zdobądź” pokazuje teraz, gdzie jest najbliższy front.",
          "Deszcz na żywo: tam, gdzie są dane o deszczu, deszcz liczy się dokładnie tam, gdzie teraz pada (ogień starzeje się szybciej, naturalne źródła ognia płoną słabiej), zamiast przez cały dzień w całym mieście.",
          "Jeśli w pobliżu jest już palenisko lub studnia, „Zdobądź” wzmacnia teraz twoje skrzyżowanie tuż przy froncie.",
        ],
      },
      it: {
        title: "Serie settimanale e pioggia dal vivo",
        items: [
          "Se giochi tre giorni in una settimana, la settimana è fatta e la tua serie cresce (sotto il tuo nome e in classifica). Un giorno di gioco: tre conquiste, una sfida, un ritrovamento o una valutazione di un luogo. Distintivi per 2, 4 e 8 settimane.",
          "Una settimana persa per campagna viene perdonata; tra le campagne la serie è in pausa.",
          "Se intorno a te è già tutto vostro, «Conquista» ora ti dice dov’è il fronte più vicino.",
          "Pioggia dal vivo: dove ci sono dati sulla pioggia, la pioggia conta esattamente dove sta piovendo (il fuoco invecchia più in fretta, le sorgenti naturali di fuoco bruciano più deboli), invece che tutto il giorno in tutta la città.",
          "Se c’è già un focolare o un pozzo vicino, «Conquista» ora rinforza un tuo incrocio proprio sul fronte.",
        ],
      },
      pt: {
        title: "Sequência semanal e chuva ao vivo",
        items: [
          "Se você jogar três dias numa semana, a semana está feita e a sua sequência cresce (abaixo do seu nome e no ranking). Um dia de jogo: três conquistas, um desafio, um achado ou uma avaliação de lugar. Insígnias para 2, 4 e 8 semanas.",
          "Uma semana perdida por campanha é perdoada; entre campanhas a sequência fica pausada.",
          "Se tudo em volta de você já é de vocês, “Conquistar” agora diz onde fica o front mais próximo.",
          "Chuva ao vivo: onde há dados de chuva, a chuva conta exatamente onde está chovendo (o fogo envelhece mais rápido, as fontes naturais de fogo queimam mais fracas), em vez de o dia todo na cidade toda.",
          "Se já houver uma fogueira ou um poço por perto, “Conquistar” agora reforça um cruzamento seu bem no front.",
        ],
      },
    },
  },
  {
    id: "2026-10-09",
    rules: true,
    text: {
      de: {
        title: "Glut und Eis einkesseln",
        items: [
          "Steht eine Glut oder ein Eis an einer Kreuzung mit mindestens drei Straßen, die zwei Minuten lang ringsum von Quellen der anderen Seite gehalten wird, ist sie eingekesselt: Sie vergeht viermal so schnell, bis der Ring bricht. Wer den Ring hält, teilt sich 50 Punkte.",
          "Eingekesselte Quellen blinken auf der Karte; ist es deine, sagt dir das Spiel Bescheid.",
          "Neu beim Setzen einer Glut oder eines Eises: Du kannst gleich eine Inschrift wählen.",
        ],
      },
      en: {
        title: "Surrounding embers and ice",
        items: [
          "If embers or ice stand at a junction with at least three roads that the other side's sources hold all around for two minutes, they are surrounded: they fade four times as fast until the ring breaks. Those holding the ring share 50 points.",
          "Surrounded sources blink on the map; if it's yours, the game tells you.",
          "New when you place embers or ice: you can pick a tag line right away.",
        ],
      },
      fr: {
        title: "Encercler braise et glace",
        items: [
          "Si une braise ou une glace se trouve à un carrefour d’au moins trois rues tenu tout autour par des sources de l’autre camp pendant deux minutes, elle est encerclée&nbsp;: elle s’éteint quatre fois plus vite jusqu’à ce que l’anneau cède. Ceux qui tiennent l’anneau se partagent 50&nbsp;points.",
          "Les sources encerclées clignotent sur la carte&nbsp;; si c’est la tienne, le jeu te prévient.",
          "Nouveau quand tu poses une braise ou une glace&nbsp;: tu peux choisir tout de suite une inscription.",
        ],
      },
      es: {
        title: "Rodear brasas y hielo",
        items: [
          "Si una brasa o un hielo está en un cruce de al menos tres calles que fuentes del otro bando rodean durante dos minutos, queda rodeado: se apaga cuatro veces más rápido hasta que se rompa el anillo. Quienes sostienen el anillo se reparten 50 puntos.",
          "Las fuentes rodeadas parpadean en el mapa; si es la tuya, el juego te avisa.",
          "Nuevo al colocar una brasa o un hielo: puedes elegir una inscripción enseguida.",
        ],
      },
      pl: {
        title: "Otaczanie żaru i lodu",
        items: [
          "Jeśli żar lub lód stoi na skrzyżowaniu co najmniej trzech ulic, które przez dwie minuty dookoła trzymają źródła drugiej strony, jest otoczony: gaśnie cztery razy szybciej, dopóki pierścień nie pęknie. Trzymający pierścień dzielą się 50 punktami.",
          "Otoczone źródła migają na mapie; jeśli to twoje, gra cię powiadomi.",
          "Nowość przy stawianiu żaru lub lodu: od razu możesz wybrać napis.",
        ],
      },
      it: {
        title: "Accerchiare Brace e Ghiaccio",
        items: [
          "Se una Brace o un Ghiaccio si trova a un incrocio di almeno tre strade tenuto tutto intorno da sorgenti dell’altra squadra per due minuti, è accerchiato: si consuma quattro volte più in fretta finché l’anello non si rompe. Chi tiene l’anello si divide 50 punti.",
          "Le sorgenti accerchiate lampeggiano sulla mappa; se è la tua, il gioco ti avvisa.",
          "Novità quando piazzi una Brace o un Ghiaccio: puoi scegliere subito un’iscrizione.",
        ],
      },
      pt: {
        title: "Cercar Brasa e Gelo",
        items: [
          "Se uma Brasa ou um Gelo está num cruzamento de pelo menos três ruas que fontes do outro time mantêm cercado por dois minutos, ele fica cercado: se desgasta quatro vezes mais rápido até o anel se romper. Quem mantém o anel divide 50 pontos.",
          "As fontes cercadas piscam no mapa; se for a sua, o jogo avisa você.",
          "Novidade ao colocar uma Brasa ou um Gelo: você já pode escolher uma inscrição na hora.",
        ],
      },
    },
  },
  {
    id: "2026-10-08-3",
    rules: true,
    text: {
      de: {
        title: "Stufen und Herausforderungen",
        items: [
          "Du sammelst Erfahrung und steigst in Stufen auf: Die Zahl auf deinem Punkt oben links ist deine Stufe.",
          "Jede Stufe bringt eine Fähigkeit (ab Stufe 2 Weitblick: eine Eroberung bis 160 m weit) und eine Glut bzw. ein Eis mehr.",
          "Das Spiel schlägt dir ab und zu eine Herausforderung vor, bis zu vier am Tag. Stufen gelten je Kampagne; Abzeichen bleiben. Mehr unter „So geht's“.",
          "Ab Stufe 2 bekommt deine Glut bzw. dein Eis eine Inschrift, die alle auf der Karte sehen: in der Rangliste unter „Meine Glut“ bzw. „Mein Eis“.",
        ],
      },
      en: {
        title: "Levels and challenges",
        items: [
          "You collect experience and rise in levels: the number on your dot at the top left is your level.",
          "Each level brings an ability (from level 2 Far sight: one claim up to 160 m away) and one more embers or ice.",
          "Now and then the game suggests a challenge, up to four a day. Levels count per campaign; badges stay. More under “How to play”.",
          "From level 2 your embers or ice can carry a tag line everyone sees on the map: in the leaderboard under “My embers” or “My ice”.",
        ],
      },
      fr: {
        title: "Niveaux et défis",
        items: [
          "Tu gagnes de l’expérience et montes de niveau&nbsp;: le chiffre sur ton point en haut à gauche est ton niveau.",
          "Chaque niveau apporte une capacité (dès le niveau 2 Longue vue&nbsp;: une conquête jusqu’à 160&nbsp;m) et une braise ou une glace de plus.",
          "De temps en temps, le jeu te propose un défi, jusqu’à quatre par jour. Les niveaux valent par campagne&nbsp;; les badges restent. Plus dans «&nbsp;Comment jouer&nbsp;».",
          "Dès le niveau 2, tes braises ou ta glace peuvent porter une inscription visible par tous sur la carte&nbsp;: dans le classement, sous «&nbsp;Mes braises&nbsp;» ou «&nbsp;Ma glace&nbsp;».",
        ],
      },
      es: {
        title: "Niveles y desafíos",
        items: [
          "Acumulas experiencia y subes de nivel: el número en tu punto arriba a la izquierda es tu nivel.",
          "Cada nivel trae una habilidad (desde el nivel 2 Vista lejana: una conquista hasta 160 m) y una brasa o un hielo más.",
          "De vez en cuando el juego te propone un desafío, hasta cuatro al día. Los niveles valen por campaña; las insignias se quedan. Más en «Cómo jugar».",
          "Desde el nivel 2 tus brasas o tu hielo pueden llevar una inscripción que todos ven en el mapa: en la clasificación, en «Mis brasas» o «Mi hielo».",
        ],
      },
      pl: {
        title: "Poziomy i wyzwania",
        items: [
          "Zbierasz doświadczenie i awansujesz: liczba na twojej kropce w lewym górnym rogu to twój poziom.",
          "Każdy poziom daje umiejętność (od poziomu 2 Dalekowzroczność: jedno zdobycie do 160 m) i jeden żar lub lód więcej.",
          "Od czasu do czasu gra proponuje wyzwanie, najwyżej cztery dziennie. Poziomy liczą się na kampanię; odznaki zostają. Więcej w „Jak grać”.",
          "Od poziomu 2 twój żar lub lód może mieć napis widoczny dla wszystkich na mapie: w rankingu pod „Mój żar” lub „Mój lód”.",
        ],
      },
    },
  },
  {
    id: "2026-10-08-2",
    rules: true,
    text: {
      de: {
        title: "Ringe brechen, zwei Dauerquellen",
        items: [
          "Bricht deine Seite den Ring einer Fläche, die die Gegenseite eine Weile gehalten hat, gibt es Punkte, je größer die Fläche, desto mehr.",
          "Du kannst jetzt zwei Gluten bzw. zwei Eis behalten. Sind beide gesetzt, fragt das Spiel beim Verlegen, welche. Deine stehen in der Rangliste unter „Meine Glut“ bzw. „Mein Eis“.",
        ],
      },
      en: {
        title: "Breaking rings, two permanent sources",
        items: [
          "If your side breaks the ring of an area the other side held for a while, you get points, more for bigger areas.",
          "You can now keep two embers or two ices. With both placed, the game asks which one to move. Yours are listed in the leaderboard under “My embers” or “My ice”.",
        ],
      },
      fr: {
        title: "Briser les anneaux, deux sources permanentes",
        items: [
          "Si ton camp brise l’anneau d’une surface que l’autre camp a tenue un moment, tu gagnes des points, plus pour les grandes surfaces.",
          "Tu peux maintenant garder deux braises ou deux glaces. Quand les deux sont posées, le jeu demande laquelle déplacer. Les tiennes figurent dans le classement sous « Mes braises » ou « Ma glace ».",
        ],
      },
      es: {
        title: "Romper anillos, dos fuentes permanentes",
        items: [
          "Si tu bando rompe el anillo de una superficie que el otro bando mantuvo un rato, ganas puntos, más cuanto más grande sea.",
          "Ahora puedes guardar dos brasas o dos hielos. Si ya están los dos, el juego pregunta cuál mover. Los tuyos aparecen en la clasificación en «Mis brasas» o «Mi hielo».",
        ],
      },
      pl: {
        title: "Przerywanie pierścieni, dwa stałe źródła",
        items: [
          "Jeśli twoja strona przerwie pierścień obszaru, który druga strona przez chwilę utrzymywała, dostajesz punkty, tym więcej, im większy obszar.",
          "Możesz teraz zachować dwa żary lub dwa lody. Gdy oba są ustawione, gra pyta, który przenieść. Twoje są w rankingu pod „Mój żar” lub „Mój lód”.",
        ],
      },
    },
  },
  {
    id: "2026-10-08",
    rules: true,
    text: {
      de: {
        title: "Flächen",
        items: [
          "Gehören die Straßen rund um einen Block (fast) ganz einer Seite, füllt sich die Fläche leicht in ihrer Farbe.",
          "Gehaltene Flächen bringen jede Minute Punkte, je größer, desto mehr. Sie gehen an alle, deren Quellen den Rand färben.",
          "Quellen am Rand einer gehaltenen Fläche verblassen nur halb so schnell. Wer einen Block schließt, muss seltener nachlegen; die Gegenseite muss den Ring aufbrechen.",
        ],
      },
      en: {
        title: "Areas",
        items: [
          "When the streets around a block (almost) all belong to one side, the area fills lightly in its colour.",
          "Held areas pay points every minute, the bigger the more. They go to everyone whose sources colour the edge.",
          "Sources on the edge of a held area fade only half as fast. Close a block and you need fewer top-ups; the other side has to break the ring.",
        ],
      },
      fr: {
        title: "Surfaces",
        items: [
          "Quand les rues autour d’un pâté de maisons appartiennent (presque) toutes à un camp, la surface se remplit légèrement de sa couleur.",
          "Les surfaces tenues rapportent des points chaque minute, d’autant plus qu’elles sont grandes. Ils vont à tous ceux dont les sources colorent le pourtour.",
          "Les sources au bord d’une surface tenue pâlissent deux fois moins vite. Qui ferme un pâté de maisons doit moins raviver&nbsp;; l’autre camp doit briser l’anneau.",
        ],
      },
      es: {
        title: "Superficies",
        items: [
          "Cuando las calles alrededor de una manzana pertenecen (casi) todas a un bando, la superficie se rellena suavemente con su color.",
          "Las superficies mantenidas dan puntos cada minuto, más cuanto más grandes. Van a todos cuyas fuentes colorean el borde.",
          "Las fuentes en el borde de una superficie mantenida se debilitan la mitad de rápido. Quien cierra una manzana necesita reforzar menos; el otro bando tiene que romper el anillo.",
        ],
      },
      pl: {
        title: "Obszary",
        items: [
          "Gdy ulice wokół kwartału należą (prawie) w całości do jednej strony, obszar lekko wypełnia się jej kolorem.",
          "Utrzymywane obszary co minutę dają punkty, tym więcej, im są większe. Dostają je wszyscy, których źródła barwią brzeg.",
          "Źródła na brzegu utrzymywanego obszaru słabną o połowę wolniej. Kto zamknie kwartał, rzadziej musi odnawiać; druga strona musi przerwać pierścień.",
        ],
      },
    },
  },
  {
    id: "2026-10-06",
    rules: true,
    text: {
      de: {
        title: "Wetter, Dauerquellen, Nachbarstädte",
        items: [
          "Das Wetter spielt mit: Regen hilft dem Wasser, Sonne und Wärme dem Feuer, Frost lässt Wasser bergauf kriechen. Das Symbol oben neben dem Fragezeichen zeigt, wem es heute hilft.",
          "Verblasste Glut und verblasstes Eis bleiben stärker und färben ihre Straßen weiter ein. Die Karte zeigt, wann sie ohne Auffrischen erlöschen.",
          "Wettertage: Die Ziele richten sich nach den letzten Wochen in deiner Stadt. Schafft keine Seite ihr Ziel, gewinnt die, die ihm näher kam.",
          "Nachbarstädte erscheinen am Kartenrand mit Grenze und Namen.",
        ],
      },
      en: {
        title: "Weather, permanent sources, neighbouring towns",
        items: [
          "The weather joins in: rain helps Water, sun and warmth help Fire, frost lets water creep uphill. The symbol at the top next to the question mark shows whom it helps today.",
          "Faded embers and ice stay stronger and keep colouring their streets. The card shows when they go out without a top-up.",
          "Weather days: the targets follow the last weeks in your town. If neither side reaches its target, the one that got closer wins.",
          "Neighbouring towns appear at the edge of the map with their border and name.",
        ],
      },
      fr: {
        title: "Météo, sources permanentes, villes voisines",
        items: [
          "La météo s’en mêle&nbsp;: la pluie aide l’Eau, le soleil et la chaleur aident le Feu, le gel fait grimper l’eau. Le symbole en haut, à côté du point d’interrogation, montre qui elle aide aujourd’hui.",
          "Les braises et la glace pâlies restent plus fortes et colorent toujours leurs rues. La fiche indique quand elles s’éteignent sans être ravivées.",
          "Jours de météo&nbsp;: les objectifs suivent les dernières semaines dans ta ville. Si aucun camp n’atteint le sien, celui qui s’en est le plus approché gagne.",
          "Les villes voisines apparaissent au bord de la carte avec leur limite et leur nom.",
        ],
      },
      es: {
        title: "Clima, fuentes permanentes, ciudades vecinas",
        items: [
          "El clima participa: la lluvia ayuda al Agua, el sol y el calor al Fuego, la helada hace que el agua suba las cuestas. El símbolo de arriba, junto al signo de interrogación, muestra a quién ayuda hoy.",
          "Las brasas y el hielo debilitados siguen siendo más fuertes y colorean sus calles. La ficha muestra cuándo se apagan si no los refuerzas.",
          "Días de tiempo: las metas siguen las últimas semanas en tu ciudad. Si ningún bando alcanza la suya, gana el que más se acercó.",
          "Las ciudades vecinas aparecen en el borde del mapa con su límite y su nombre.",
        ],
      },
      pl: {
        title: "Pogoda, stałe źródła, sąsiednie miasta",
        items: [
          "Pogoda ma znaczenie: deszcz pomaga Wodzie, słońce i ciepło Ogniowi, mróz sprawia, że woda pnie się pod górę. Symbol u góry obok znaku zapytania pokazuje, komu dziś pomaga.",
          "Osłabiony żar i lód pozostają silniejsze i nadal barwią swoje ulice. Karta pokazuje, kiedy zgasną bez odnowienia.",
          "Dni pogody: cele zależą od ostatnich tygodni w twoim mieście. Jeśli żadna strona nie osiągnie celu, wygrywa ta, która była bliżej.",
          "Sąsiednie miasta widać na skraju mapy z granicą i nazwą.",
        ],
      },
    },
  },
];

const KEY = KEYS.newsSeen;

function seen(id: string) {
  save(KEY, id);
}

/** Played before: the intro was seen on an earlier visit (main.ts INTRO_KEY), read before this visit's intro. */
const returning = load(KEYS.intro) !== null;

export function setupNews(t: (k: "news.title" | "news.rules" | "notice.more" | "news.chron") => string) {
  const box = $("news");
  const latest = NEWS[0]?.id ?? "";

  function show(entries: Entry[]) {
    const notices = activeNotices();
    if (!entries.length && !notices.length) return;
    const list = $("news-list");
    // The chronicle first: what happened in the campaigns.
    const chron = document.createElement("p");
    chron.className = "news-chron";
    chron.append(Object.assign(document.createElement("a"), { href: withRegion("/chronik.html"), textContent: t("news.chron") }));
    list.replaceChildren(
      chron,
      // The city's news flashes first (notices.ts).
      ...notices.map((n) => {
        const x = noticeText(n);
        const sec = document.createElement("section");
        sec.className = "news-notice";
        const h = document.createElement("h3");
        h.textContent = x.title;
        sec.append(h, ...paragraphs(x.text));
        if (n.url) {
          const p = document.createElement("p");
          const a = Object.assign(document.createElement("a"), { href: n.url, textContent: t("notice.more"), target: "_blank", rel: "noopener noreferrer" });
          p.append(a);
          sec.append(p);
        }
        return sec;
      }),
      ...entries.map((e) => {
        const x = e.text[lang] ?? e.text.en ?? e.text.de;
        const sec = document.createElement("section");
        const h = document.createElement("h3");
        h.textContent = x.title;
        if (e.rules) {
          const tag = document.createElement("span");
          tag.className = "news-rules";
          tag.textContent = t("news.rules");
          h.append(" ", tag);
        }
        const ul = document.createElement("ul");
        for (const it of x.items) {
          const li = document.createElement("li");
          // Entities only (&nbsp;), no markup.
          li.innerHTML = it.replace(/</g, "&lt;");
          ul.append(li);
        }
        sec.append(h, ul);
        return sec;
      }),
    );
    box.hidden = false;
    seen(latest);
  }

  $("news-close").onclick = () => (box.hidden = true);
  $("news-open").onclick = () => {
    ($("board-close") as HTMLButtonElement).click();
    show(NEWS.slice(0, 3));
  };

  let checked = false;
  return {
    /** After the first welcome: unseen entries, once. */
    check() {
      if (checked) return;
      checked = true;
      const last = load(KEY);
      // New here (no game yet): nothing to catch up on.
      if (last === null && !returning) return seen(latest);
      const fresh = last === null ? NEWS.slice(0, 1) : NEWS.filter((e) => e.id > last);
      show(fresh.slice(0, 3));
    },
  };
}
