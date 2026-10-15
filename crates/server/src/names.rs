//! Random player names in the region's language: an adjective in the spirit
//! of the side (fire: glowing, water: calm) and an animal, with the grammar of
//! each language (gender agreement, word order), plus a number.

use elements_shared::prelude::Faction;
use rand::seq::IndexedRandom;

/// Grammatical gender of a noun.
#[derive(Clone, Copy)]
enum G {
    M,
    F,
    N,
}

/// Adjective forms (masculine, feminine, neuter).
type Adj = (&'static str, &'static str, &'static str);

struct Lang {
    fire: &'static [Adj],
    water: &'static [Adj],
    animals: &'static [(&'static str, G)],
    /// Adjective after the noun (es, fr, it, pt).
    after: bool,
}

const DE: Lang = Lang {
    fire: &[
        ("Glühender", "Glühende", "Glühendes"),
        ("Lodernder", "Lodernde", "Loderndes"),
        ("Funkelnder", "Funkelnde", "Funkelndes"),
        ("Feuriger", "Feurige", "Feuriges"),
        ("Knisternder", "Knisternde", "Knisterndes"),
        ("Glimmender", "Glimmende", "Glimmendes"),
        ("Strahlender", "Strahlende", "Strahlendes"),
        ("Flinker", "Flinke", "Flinkes"),
        ("Heißer", "Heiße", "Heißes"),
        ("Wilder", "Wilde", "Wildes"),
    ],
    water: &[
        ("Stiller", "Stille", "Stilles"),
        ("Tiefer", "Tiefe", "Tiefes"),
        ("Klarer", "Klare", "Klares"),
        ("Perlender", "Perlende", "Perlendes"),
        ("Sprudelnder", "Sprudelnde", "Sprudelndes"),
        ("Kühler", "Kühle", "Kühles"),
        ("Sanfter", "Sanfte", "Sanftes"),
        ("Fließender", "Fließende", "Fließendes"),
        ("Glitzernder", "Glitzernde", "Glitzerndes"),
        ("Ruhiger", "Ruhige", "Ruhiges"),
    ],
    animals: &[
        ("Fuchs", G::M),
        ("Hecht", G::M),
        ("Schwan", G::M),
        ("Igel", G::M),
        ("Dachs", G::M),
        ("Otter", G::M),
        ("Kormoran", G::M),
        ("Rabe", G::M),
        ("Biber", G::M),
        ("Haubentaucher", G::M),
        ("Möwe", G::F),
        ("Ente", G::F),
        ("Eule", G::F),
        ("Katze", G::F),
        ("Libelle", G::F),
        ("Taube", G::F),
        ("Felchen", G::N),
        ("Reh", G::N),
        ("Eichhörnchen", G::N),
        ("Blässhuhn", G::N),
    ],
    after: false,
};

const EN: Lang = Lang {
    fire: &[
        ("Glowing", "Glowing", "Glowing"),
        ("Blazing", "Blazing", "Blazing"),
        ("Sparkling", "Sparkling", "Sparkling"),
        ("Fiery", "Fiery", "Fiery"),
        ("Crackling", "Crackling", "Crackling"),
        ("Smouldering", "Smouldering", "Smouldering"),
        ("Radiant", "Radiant", "Radiant"),
        ("Swift", "Swift", "Swift"),
        ("Hot", "Hot", "Hot"),
        ("Wild", "Wild", "Wild"),
    ],
    water: &[
        ("Still", "Still", "Still"),
        ("Deep", "Deep", "Deep"),
        ("Clear", "Clear", "Clear"),
        ("Pearly", "Pearly", "Pearly"),
        ("Bubbling", "Bubbling", "Bubbling"),
        ("Cool", "Cool", "Cool"),
        ("Gentle", "Gentle", "Gentle"),
        ("Flowing", "Flowing", "Flowing"),
        ("Glittering", "Glittering", "Glittering"),
        ("Calm", "Calm", "Calm"),
    ],
    animals: &[
        ("Fox", G::N),
        ("Pike", G::N),
        ("Swan", G::N),
        ("Hedgehog", G::N),
        ("Badger", G::N),
        ("Otter", G::N),
        ("Cormorant", G::N),
        ("Raven", G::N),
        ("Beaver", G::N),
        ("Grebe", G::N),
        ("Gull", G::N),
        ("Duck", G::N),
        ("Owl", G::N),
        ("Cat", G::N),
        ("Dragonfly", G::N),
        ("Dove", G::N),
        ("Heron", G::N),
        ("Deer", G::N),
        ("Squirrel", G::N),
        ("Coot", G::N),
    ],
    after: false,
};

const ES: Lang = Lang {
    fire: &[
        ("Ardiente", "Ardiente", ""),
        ("Llameante", "Llameante", ""),
        ("Chispeante", "Chispeante", ""),
        ("Fogoso", "Fogosa", ""),
        ("Crepitante", "Crepitante", ""),
        ("Radiante", "Radiante", ""),
        ("Brillante", "Brillante", ""),
        ("Veloz", "Veloz", ""),
        ("Ígneo", "Ígnea", ""),
        ("Salvaje", "Salvaje", ""),
    ],
    water: &[
        ("Sereno", "Serena", ""),
        ("Profundo", "Profunda", ""),
        ("Claro", "Clara", ""),
        ("Burbujeante", "Burbujeante", ""),
        ("Espumoso", "Espumosa", ""),
        ("Fresco", "Fresca", ""),
        ("Suave", "Suave", ""),
        ("Cristalino", "Cristalina", ""),
        ("Fluido", "Fluida", ""),
        ("Tranquilo", "Tranquila", ""),
    ],
    animals: &[
        ("Zorro", G::M),
        ("Cisne", G::M),
        ("Erizo", G::M),
        ("Tejón", G::M),
        ("Cormorán", G::M),
        ("Cuervo", G::M),
        ("Castor", G::M),
        ("Pato", G::M),
        ("Gato", G::M),
        ("Canario", G::M),
        ("Colibrí", G::M),
        ("Delfín", G::M),
        ("Nutria", G::F),
        ("Gaviota", G::F),
        ("Lechuza", G::F),
        ("Libélula", G::F),
        ("Paloma", G::F),
        ("Ardilla", G::F),
        ("Garza", G::F),
        ("Tortuga", G::F),
    ],
    after: true,
};

const FR: Lang = Lang {
    fire: &[
        ("Ardent", "Ardente", ""),
        ("Flamboyant", "Flamboyante", ""),
        ("Pétillant", "Pétillante", ""),
        ("Fougueux", "Fougueuse", ""),
        ("Crépitant", "Crépitante", ""),
        ("Rayonnant", "Rayonnante", ""),
        ("Brûlant", "Brûlante", ""),
        ("Vif", "Vive", ""),
        ("Rapide", "Rapide", ""),
        ("Sauvage", "Sauvage", ""),
    ],
    water: &[
        ("Calme", "Calme", ""),
        ("Profond", "Profonde", ""),
        ("Clair", "Claire", ""),
        ("Perlé", "Perlée", ""),
        ("Bouillonnant", "Bouillonnante", ""),
        ("Frais", "Fraîche", ""),
        ("Doux", "Douce", ""),
        ("Fluide", "Fluide", ""),
        ("Scintillant", "Scintillante", ""),
        ("Paisible", "Paisible", ""),
    ],
    animals: &[
        ("Renard", G::M),
        ("Brochet", G::M),
        ("Cygne", G::M),
        ("Hérisson", G::M),
        ("Blaireau", G::M),
        ("Cormoran", G::M),
        ("Corbeau", G::M),
        ("Castor", G::M),
        ("Canard", G::M),
        ("Chat", G::M),
        ("Chevreuil", G::M),
        ("Écureuil", G::M),
        ("Loutre", G::F),
        ("Mouette", G::F),
        ("Chouette", G::F),
        ("Libellule", G::F),
        ("Colombe", G::F),
        ("Foulque", G::F),
        ("Hirondelle", G::F),
        ("Grenouille", G::F),
    ],
    after: true,
};

const IT: Lang = Lang {
    fire: &[
        ("Ardente", "Ardente", ""),
        ("Rovente", "Rovente", ""),
        ("Scintillante", "Scintillante", ""),
        ("Fiammeggiante", "Fiammeggiante", ""),
        ("Focoso", "Focosa", ""),
        ("Infuocato", "Infuocata", ""),
        ("Radioso", "Radiosa", ""),
        ("Luminoso", "Luminosa", ""),
        ("Veloce", "Veloce", ""),
        ("Selvaggio", "Selvaggia", ""),
    ],
    water: &[
        ("Limpido", "Limpida", ""),
        ("Fresco", "Fresca", ""),
        ("Spumeggiante", "Spumeggiante", ""),
        ("Sereno", "Serena", ""),
        ("Profondo", "Profonda", ""),
        ("Cristallino", "Cristallina", ""),
        ("Frizzante", "Frizzante", ""),
        ("Calmo", "Calma", ""),
        ("Gentile", "Gentile", ""),
        ("Tranquillo", "Tranquilla", ""),
    ],
    animals: &[
        ("Gatto", G::M),
        ("Lupo", G::M),
        ("Riccio", G::M),
        ("Gabbiano", G::M),
        ("Cinghiale", G::M),
        ("Fenicottero", G::M),
        ("Muflone", G::M),
        ("Delfino", G::M),
        ("Cigno", G::M),
        ("Tasso", G::M),
        ("Castoro", G::M),
        ("Scoiattolo", G::M),
        ("Volpe", G::F),
        ("Lontra", G::F),
        ("Civetta", G::F),
        ("Libellula", G::F),
        ("Colomba", G::F),
        ("Rondine", G::F),
        ("Tartaruga", G::F),
        ("Rana", G::F),
    ],
    after: true,
};

const PT: Lang = Lang {
    fire: &[
        ("Ardente", "Ardente", ""),
        ("Flamejante", "Flamejante", ""),
        ("Faiscante", "Faiscante", ""),
        ("Fogoso", "Fogosa", ""),
        ("Crepitante", "Crepitante", ""),
        ("Radiante", "Radiante", ""),
        ("Brilhante", "Brilhante", ""),
        ("Veloz", "Veloz", ""),
        ("Ígneo", "Ígnea", ""),
        ("Selvagem", "Selvagem", ""),
    ],
    water: &[
        ("Sereno", "Serena", ""),
        ("Profundo", "Profunda", ""),
        ("Claro", "Clara", ""),
        ("Borbulhante", "Borbulhante", ""),
        ("Espumante", "Espumante", ""),
        ("Fresco", "Fresca", ""),
        ("Suave", "Suave", ""),
        ("Cristalino", "Cristalina", ""),
        ("Fluido", "Fluida", ""),
        ("Tranquilo", "Tranquila", ""),
    ],
    animals: &[
        ("Tucano", G::M),
        ("Jacaré", G::M),
        ("Boto", G::M),
        ("Sabiá", G::M),
        ("Quati", G::M),
        ("Tatu", G::M),
        ("Mico", G::M),
        ("Gato", G::M),
        ("Cisne", G::M),
        ("Castor", G::M),
        ("Pato", G::M),
        ("Golfinho", G::M),
        ("Arara", G::F),
        ("Onça", G::F),
        ("Capivara", G::F),
        ("Coruja", G::F),
        ("Lontra", G::F),
        ("Tartaruga", G::F),
        ("Gaivota", G::F),
        ("Raposa", G::F),
    ],
    after: true,
};

const PL: Lang = Lang {
    fire: &[
        ("Ognisty", "Ognista", ""),
        ("Płonący", "Płonąca", ""),
        ("Iskrzący", "Iskrząca", ""),
        ("Żarliwy", "Żarliwa", ""),
        ("Trzaskający", "Trzaskająca", ""),
        ("Tlący", "Tląca", ""),
        ("Promienny", "Promienna", ""),
        ("Zwinny", "Zwinna", ""),
        ("Gorący", "Gorąca", ""),
        ("Dziki", "Dzika", ""),
    ],
    water: &[
        ("Cichy", "Cicha", ""),
        ("Głęboki", "Głęboka", ""),
        ("Czysty", "Czysta", ""),
        ("Perlisty", "Perlista", ""),
        ("Musujący", "Musująca", ""),
        ("Chłodny", "Chłodna", ""),
        ("Łagodny", "Łagodna", ""),
        ("Płynący", "Płynąca", ""),
        ("Lśniący", "Lśniąca", ""),
        ("Spokojny", "Spokojna", ""),
    ],
    animals: &[
        ("Lis", G::M),
        ("Szczupak", G::M),
        ("Łabędź", G::M),
        ("Jeż", G::M),
        ("Borsuk", G::M),
        ("Kormoran", G::M),
        ("Kruk", G::M),
        ("Bóbr", G::M),
        ("Perkoz", G::M),
        ("Dzięcioł", G::M),
        ("Mewa", G::F),
        ("Kaczka", G::F),
        ("Sowa", G::F),
        ("Wydra", G::F),
        ("Ważka", G::F),
        ("Wiewiórka", G::F),
        ("Łyska", G::F),
        ("Czapla", G::F),
        ("Foka", G::F),
        ("Sarna", G::F),
    ],
    after: false,
};

fn lang(code: &str) -> &'static Lang {
    match code {
        "en" => &EN,
        "es" => &ES,
        "fr" => &FR,
        "it" => &IT,
        "pl" => &PL,
        "pt" => &PT,
        _ => &DE,
    }
}

/// A new name for a player of side `f` in language `code` (German if unknown).
pub fn random_name(f: Faction, code: &str) -> String {
    let l = lang(code);
    let mut rng = rand::rng();
    let adjs = match f {
        Faction::Fire => l.fire,
        Faction::Water => l.water,
    };
    let (m, fem, n) = *adjs.choose(&mut rng).unwrap();
    let (noun, g) = *l.animals.choose(&mut rng).unwrap();
    let adj = match g {
        G::M => m,
        G::F => fem,
        G::N => n,
    };
    let num = rand::random_range(1..100);
    if l.after {
        format!("{noun} {adj} {num}")
    } else {
        format!("{adj} {noun} {num}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_agrees() {
        for code in ["de", "en", "es", "fr", "it", "pl", "pt", "xx"] {
            let l = lang(code);
            for (noun, g) in l.animals {
                for (m, f, n) in l.fire.iter().chain(l.water) {
                    let adj = match g {
                        G::M => m,
                        G::F => f,
                        G::N => n,
                    };
                    assert!(!adj.is_empty(), "{code}: no form for {noun}");
                }
            }
            let name = random_name(Faction::Fire, code);
            assert!(name.split(' ').count() == 3, "{name}");
        }
        let es = lang("es");
        assert!(es.after && ES.animals.iter().any(|(a, _)| *a == "Canario"));
        let it = lang("it");
        assert!(it.after && IT.animals.iter().any(|(a, _)| *a == "Volpe"));
        let pt = lang("pt");
        assert!(pt.after && PT.animals.iter().any(|(a, _)| *a == "Capivara"));
    }
}
